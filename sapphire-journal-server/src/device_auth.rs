//! デバイス台帳を通した認証。
//!
//! framework の `protect` は `KeyStore` しか見ず、認証結果を extensions に
//! 挿すのは**内側**なので、その値を読むレイヤを外から張ることはできない。
//! よってここは Bearer を自分で読み、鍵 → `device_id` → 台帳の行、まで解決
//! できたリクエストだけを通す。
//!
//! これは起動時のスナップショット。`KeyStore` が既にそうであるように、
//! 再読み込みの経路は持たない —— `device rotate` / `device retire` が動いて
//! いるサーバに効くのは次の起動時。

use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use sapphire_framework::registry::{Device, Devices};
use sapphire_framework::remote_server::KeyStore;

pub struct DeviceAuth {
    keys: KeyStore,
    devices: Devices,
}

impl DeviceAuth {
    /// 鍵ファイルと台帳を読む。どちらも存在しなければ空として扱う
    /// （`KeyStore::load` / `Devices::load` の既定）。
    pub fn load(keys_path: &Path, devices_path: &Path) -> anyhow::Result<Self> {
        let keys = KeyStore::load(keys_path)
            .with_context(|| format!("loading API keys from {}", keys_path.display()))?;
        let devices = Devices::load(devices_path)
            .with_context(|| format!("loading device table {}", devices_path.display()))?;
        Ok(Self { keys, devices })
    }

    /// トークン → デバイス。
    ///
    /// 失敗理由（鍵が無い・期限切れ・`device_id` が無い・行が無い・引退済み）は
    /// すべて `None` に潰す。呼び出し側は全部に 401 を返し、区別はログにだけ
    /// 出す —— どの段階で落ちたかを返すと、鍵の有無を試せる口になる。
    pub fn resolve(&self, token: &str) -> Option<&Device> {
        let entry = self.keys.authenticate(token)?;
        let Some(device_id) = entry.device_id else {
            tracing::debug!(key_id = %entry.id, "key has no device_id; refusing");
            return None;
        };
        let Some(device) = self.devices.get(device_id) else {
            tracing::debug!(%device_id, "key names a device that is not in the table; refusing");
            return None;
        };
        if device.is_retired() {
            tracing::debug!(%device_id, "device is retired; refusing");
            return None;
        }
        Some(device)
    }

    /// 生きたデバイスを指す、期限切れでない鍵が 1 本以上あるか。
    ///
    /// 起動ガードが使う。0 本で待ち受けるのは、鍵が 0 本のときと同じ事故
    /// （誰も繋がらないサーバが黙って上がる）。
    pub fn has_usable_device_key(&self) -> bool {
        let now = chrono::Utc::now();
        self.keys.entries().iter().any(|k| {
            !k.is_expired(now)
                && k.device_id
                    .and_then(|id| self.devices.get(id))
                    .is_some_and(|d| !d.is_retired())
        })
    }

    /// 生きたデバイス行に辿り着けない鍵の本数。
    ///
    /// 数えるのは 3 種類 —— `device_id` を持たない鍵、台帳に無いデバイスを
    /// 指す鍵、そして**引退済みの行**を指す鍵。どれも [`Self::resolve`] が
    /// 必ず弾くので、「認証先が無い」という意味では同じ。引退済みを外すと、
    /// 同期で引退が届いたホストで死んだ鍵が見えなくなる。
    ///
    /// 期限切れは数えない。行は生きていて `device rotate` で戻せる ——
    /// 掃除の対象ではない。
    ///
    /// `list-keys` を消した以上、放置された旧鍵に気づく口はここしか無い。
    /// 起動時に warn へ出す。
    pub fn orphan_key_count(&self) -> usize {
        self.keys
            .entries()
            .iter()
            .filter(|k| {
                k.device_id
                    .and_then(|id| self.devices.get(id))
                    .is_none_or(Device::is_retired)
            })
            .count()
    }
}

/// `router` にデバイス検査を被せる。
///
/// framework の `protect` は残したまま、その**外側**に置く。内側と外側で鍵を
/// 2 回引くことになるが、`protect` を外すと「レイヤの順序を間違えると素通しに
/// なる」構成を自前で抱えることになる。
///
/// 経路が増えたときの取りこぼしを避けるため、`/rpc` と `/mcp` を merge した
/// **後**の Router に 1 回だけ被せること。
pub fn require_device(auth: Arc<DeviceAuth>, router: Router) -> Router {
    router.layer(from_fn_with_state(auth, check))
}

async fn check(
    State(auth): State<Arc<DeviceAuth>>,
    request: Request,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    // `request` を `next` に渡すので、ヘッダの借用はここで閉じる。
    let allowed = {
        let presented = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match presented {
            Some(token) => auth.resolve(token).is_some(),
            None => {
                tracing::debug!("no bearer token; refusing");
                false
            }
        }
    };
    if !allowed {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_framework::registry::Devices;
    use sapphire_framework::remote_server::KeyStore;

    use super::*;
    use crate::cli_device::{DeviceCommand, run_device};

    struct Files {
        _dir: tempfile::TempDir,
        devices: PathBuf,
        users: PathBuf,
        keys: PathBuf,
    }

    fn files() -> Files {
        let dir = tempfile::tempdir().unwrap();
        Files {
            devices: dir.path().join("devices.toml"),
            users: dir.path().join("users.toml"),
            keys: dir.path().join("keys.toml"),
            _dir: dir,
        }
    }

    fn add(f: &Files, name: &str) -> String {
        run_device(
            DeviceCommand::Add {
                name: name.into(),
                description: None,
                user: None,
                expires_in: None,
            },
            &f.devices,
            &f.users,
            &f.keys,
        )
        .unwrap();
        KeyStore::load(&f.keys)
            .unwrap()
            .entries()
            .iter()
            .find(|k| k.label.as_deref() == Some(name))
            .unwrap()
            .token
            .clone()
    }

    fn auth(f: &Files) -> DeviceAuth {
        DeviceAuth::load(&f.keys, &f.devices).unwrap()
    }

    #[test]
    fn a_live_device_resolves() {
        let f = files();
        let token = add(&f, "laptop");

        // `resolve` は借用を返すので、`DeviceAuth` を式の途中で落とせない。
        let auth = auth(&f);
        let device = auth.resolve(&token).expect("生きたデバイスが弾かれた");

        assert_eq!(device.name, "laptop");
    }

    #[test]
    fn an_unknown_token_does_not_resolve() {
        let f = files();
        add(&f, "laptop");

        assert!(auth(&f).resolve("sjt_nope").is_none());
    }

    /// 移行前に `gen-key` で作られた鍵。台帳を経由しないので通さない。
    #[test]
    fn a_key_without_a_device_id_does_not_resolve() {
        let f = files();
        let mut keys = KeyStore::load(&f.keys).unwrap();
        let token = keys
            .generate(crate::keys::TOKEN_PREFIX, None, None, Some("old".into()), None)
            .unwrap()
            .token;

        assert!(auth(&f).resolve(&token).is_none());
    }

    #[test]
    fn a_key_pointing_at_a_missing_row_does_not_resolve() {
        let f = files();
        let token = add(&f, "laptop");
        // 鍵はそのまま、台帳の行だけ消す（他ホストから同期された削除の再現）。
        Devices::load(&f.devices).unwrap().purge("laptop").unwrap();

        assert!(auth(&f).resolve(&token).is_none());
    }

    /// `device retire` は鍵も失効させるので、この状態は手編集か同期でしか
    /// 起きない。それでも通してはいけない。
    #[test]
    fn a_retired_device_does_not_resolve() {
        let f = files();
        let token = add(&f, "laptop");
        Devices::load(&f.devices).unwrap().retire("laptop").unwrap();

        assert!(auth(&f).resolve(&token).is_none());
    }

    /// 期限切れの鍵は、行が生きていても通さない。
    ///
    /// `device add --expires-in` は最短でも 1 分先しか指定できない（0 と負は
    /// `parse_duration` が拒否する）ので、待たずに作るには過去の期限を直接
    /// 渡して発行する。
    #[test]
    fn an_expired_key_does_not_resolve() {
        let f = files();
        let device_id = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", None, None)
            .unwrap()
            .id;
        let mut keys = KeyStore::load(&f.keys).unwrap();
        let token = keys
            .generate(
                crate::keys::TOKEN_PREFIX,
                None,
                Some(device_id),
                Some("laptop".into()),
                Some(chrono::Utc::now() - chrono::Duration::hours(1)),
            )
            .unwrap()
            .token;

        assert!(auth(&f).resolve(&token).is_none());
    }

    #[test]
    fn has_usable_device_key_is_false_without_a_live_device() {
        let f = files();
        assert!(!auth(&f).has_usable_device_key(), "鍵も台帳も無い");

        let mut keys = KeyStore::load(&f.keys).unwrap();
        keys.generate(crate::keys::TOKEN_PREFIX, None, None, Some("old".into()), None)
            .unwrap();
        assert!(
            !auth(&f).has_usable_device_key(),
            "台帳を経由しない鍵で起動できてしまう"
        );
        assert_eq!(auth(&f).orphan_key_count(), 1);

        add(&f, "laptop");
        assert!(auth(&f).has_usable_device_key());
        assert_eq!(auth(&f).orphan_key_count(), 1, "孤児の鍵は数え続ける");
    }

    /// 引退済みの行を指す鍵も「認証先が無い」。`device retire` は鍵も消すので
    /// この状態は同期か手編集でしか起きないが、そのとき運用者に見える口は
    /// この警告しか無い。
    #[test]
    fn orphan_key_count_includes_keys_pointing_at_a_retired_row() {
        let f = files();
        add(&f, "laptop");
        assert_eq!(auth(&f).orphan_key_count(), 0, "生きた行を指す鍵は孤児ではない");

        // 行だけ引退させる（他ホストからの同期の再現）。鍵は残る。
        Devices::load(&f.devices).unwrap().retire("laptop").unwrap();

        assert_eq!(KeyStore::load(&f.keys).unwrap().entries().len(), 1);
        assert_eq!(
            auth(&f).orphan_key_count(),
            1,
            "引退済みの行を指す鍵が数えられていない"
        );
    }

    /// 期限切れは孤児ではない。行は生きていて `device rotate` で戻せる。
    #[test]
    fn orphan_key_count_does_not_count_an_expired_key_on_a_live_row() {
        let f = files();
        let device_id = Devices::load(&f.devices)
            .unwrap()
            .add("laptop", None, None)
            .unwrap()
            .id;
        KeyStore::load(&f.keys)
            .unwrap()
            .generate(
                crate::keys::TOKEN_PREFIX,
                None,
                Some(device_id),
                Some("laptop".into()),
                Some(chrono::Utc::now() - chrono::Duration::hours(1)),
            )
            .unwrap();

        assert_eq!(auth(&f).orphan_key_count(), 0);
    }
}
