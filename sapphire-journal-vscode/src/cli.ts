import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';

let _extensionPath: string | undefined;

/**
 * Build environment variable overrides from VSCode settings.
 *
 * Any `sapphire-journal.cache.*` setting with a non-empty value is translated to the
 * corresponding `ARCHELON_CACHE_*` environment variable so that the CLI treats
 * VSCode settings as overrides on top of `config.toml`.
 */
export function configEnv(): Record<string, string> {
    const cfg = vscode.workspace.getConfiguration('sapphire-journal');
    const env: Record<string, string> = {};

    const vectorDb = cfg.get<string>('cache.vectorDb', '');
    if (vectorDb) { env['ARCHELON_CACHE_VECTOR_DB'] = vectorDb; }

    const provider = cfg.get<string>('cache.embedding.provider', '');
    if (provider) { env['ARCHELON_CACHE_EMBEDDING_PROVIDER'] = provider; }

    const model = cfg.get<string>('cache.embedding.model', '');
    if (model) { env['ARCHELON_CACHE_EMBEDDING_MODEL'] = model; }

    const apiKeyEnv = cfg.get<string>('cache.embedding.apiKeyEnv', '');
    if (apiKeyEnv) { env['ARCHELON_CACHE_EMBEDDING_API_KEY_ENV'] = apiKeyEnv; }

    const baseUrl = cfg.get<string>('cache.embedding.baseUrl', '');
    if (baseUrl) { env['ARCHELON_CACHE_EMBEDDING_BASE_URL'] = baseUrl; }

    const dimension = cfg.get<string>('cache.embedding.dimension', '');
    if (dimension) { env['ARCHELON_CACHE_EMBEDDING_DIMENSION'] = dimension; }

    return env;
}

export function setExtensionPath(p: string) {
    _extensionPath = p;
}

export function bin(): string {
    const configured = vscode.workspace.getConfiguration('sapphire-journal').get<string>('binaryPath', '');
    if (configured) { return configured; }

    if (_extensionPath) {
        const ext = process.platform === 'win32' ? '.exe' : '';
        const bundled = path.join(_extensionPath, 'bin', `sapphire-journal${ext}`);
        if (fs.existsSync(bundled)) { return bundled; }
    }

    return 'sapphire-journal';
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
