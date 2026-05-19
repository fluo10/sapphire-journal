import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';

let _extensionPath: string | undefined;

/**
 * Build environment variable overrides from VSCode settings.
 *
 * Any `sapphire-journal.*` setting with a non-default value is translated to
 * the corresponding `SAPPHIRE_JOURNAL_*` environment variable so that the CLI
 * treats VSCode settings as overrides on top of `config.toml`.
 */
export function configEnv(): Record<string, string> {
    const cfg = vscode.workspace.getConfiguration('sapphire-journal');
    const env: Record<string, string> = {};

    const vectorDb = cfg.get<string>('cache.vectorDb', '');
    if (vectorDb) { env['SAPPHIRE_JOURNAL_CACHE_RETRIEVE_DB'] = vectorDb; }

    const embeddingEnabled = cfg.get<boolean | null>('cache.embedding.enabled', null);
    if (embeddingEnabled !== null && embeddingEnabled !== undefined) {
        env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_ENABLED'] = embeddingEnabled ? 'true' : 'false';
    }

    const provider = cfg.get<string>('cache.embedding.provider', '');
    if (provider) { env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_PROVIDER'] = provider; }

    const model = cfg.get<string>('cache.embedding.model', '');
    if (model) { env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_MODEL'] = model; }

    const apiKeyEnv = cfg.get<string>('cache.embedding.apiKeyEnv', '');
    if (apiKeyEnv) { env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_API_KEY_ENV'] = apiKeyEnv; }

    const baseUrl = cfg.get<string>('cache.embedding.baseUrl', '');
    if (baseUrl) { env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_BASE_URL'] = baseUrl; }

    const dimension = cfg.get<string>('cache.embedding.dimension', '');
    if (dimension) { env['SAPPHIRE_JOURNAL_CACHE_EMBEDDING_DIMENSION'] = dimension; }

    const syncBackend = cfg.get<string>('sync.backend', '');
    if (syncBackend) { env['SAPPHIRE_JOURNAL_SYNC_BACKEND'] = syncBackend; }

    const syncIntervalMinutes = cfg.get<number | null>('syncIntervalMinutes', null);
    if (syncIntervalMinutes !== null && syncIntervalMinutes !== undefined) {
        env['SAPPHIRE_JOURNAL_SYNC_INTERVAL_MINUTES'] = String(syncIntervalMinutes);
    }

    return env;
}

export function setExtensionPath(p: string) {
    _extensionPath = p;
}

/**
 * Resolve how to launch the MCP server.
 *
 * Preference order:
 *   1. `sapphire-journal.mcpBinaryPath` — explicit path to a `sapphire-journal-mcp` binary.
 *   2. Bundled `bin/sapphire-journal-mcp` shipped with the extension.
 *   3. Legacy `sapphire-journal.binaryPath` — path to a `sajo` binary;
 *      invoked as `sajo mcp` for backward compat with pre-split installs.
 *   4. `sapphire-journal-mcp` on $PATH.
 */
export function mcpLauncher(): { command: string; args: string[] } {
    const cfg = vscode.workspace.getConfiguration('sapphire-journal');

    const mcpConfigured = cfg.get<string>('mcpBinaryPath', '');
    if (mcpConfigured) { return { command: mcpConfigured, args: [] }; }

    if (_extensionPath) {
        const ext = process.platform === 'win32' ? '.exe' : '';
        const bundled = path.join(_extensionPath, 'bin', `sapphire-journal-mcp${ext}`);
        if (fs.existsSync(bundled)) { return { command: bundled, args: [] }; }
    }

    const legacyConfigured = cfg.get<string>('binaryPath', '');
    if (legacyConfigured) { return { command: legacyConfigured, args: ['mcp'] }; }

    return { command: 'sapphire-journal-mcp', args: [] };
}

export interface EntryRecord {
    id: string;
    path: string;
    title: string;
    tags: string[];
    updated_at: string;
    task?: { status: string; due?: string; closed_at?: string } | null;
    event?: { start: string; end: string } | null;
    flags?: string[];
    children?: EntryRecord[];
}

export type SortField = 'id' | 'title' | 'task_status' | 'created_at' | 'updated_at' | 'task_due' | 'event_start';
export type SortOrder = 'asc' | 'desc';
