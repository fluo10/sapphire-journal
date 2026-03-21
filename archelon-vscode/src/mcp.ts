import * as path from 'path';
import * as vscode from 'vscode';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { findJournalRoot } from './journal';

// Re-export types so callers can import from a single place.
export type { EntryRecord, SortField, SortOrder } from './cli';

export class ArchelonMcpClient implements vscode.Disposable {
    private readonly _client: Client;
    private readonly _transport: StdioClientTransport;
    private _openedJournalDir: string | null = null;

    constructor(binPath: string, workspaceRoot?: string) {
        this._transport = new StdioClientTransport({
            command: binPath,
            args: ['mcp'],
            // Set cwd so Journal::find() can auto-discover a journal at the workspace root.
            ...(workspaceRoot ? { cwd: workspaceRoot } : {}),
        });
        this._client = new Client(
            { name: 'archelon-vscode', version: '1.0.0' },
            { capabilities: {} },
        );
    }

    async connect(): Promise<void> {
        await this._client.connect(this._transport);
    }

    // ── private helpers ──────────────────────────────────────────────────────

    private async callTool(name: string, args: Record<string, unknown>): Promise<string> {
        const result = await this._client.callTool({ name, arguments: args });
        const content = result.content as Array<{ type: string; text?: string }>;
        const text = content
            .filter(c => c.type === 'text' && c.text !== undefined)
            .map(c => c.text as string)
            .join('\n');
        if (result.isError) {
            throw new Error(text);
        }
        return text;
    }

    /** Call journal_open only when the journal directory changes. */
    private async ensureJournal(journalDir: string): Promise<void> {
        if (this._openedJournalDir !== journalDir) {
            await this.callTool('journal_open', { path: journalDir });
            this._openedJournalDir = journalDir;
        }
    }

    // ── public API (mirrors cli.ts) ──────────────────────────────────────────

    async init(dirPath: string): Promise<string> {
        const result = await this.callTool('journal_init', { path: dirPath });
        this._openedJournalDir = null; // force re-open on next use
        return result;
    }

    async cacheRebuild(cwd: string): Promise<string> {
        await this.ensureJournal(cwd);
        return this.callTool('cache_rebuild', {});
    }

    /**
     * Normalize an entry file. Returns the new absolute path if the file was
     * renamed, or null if it stayed in place.
     */
    async fixEntry(filePath: string): Promise<string | null> {
        const journalRoot = findJournalRoot(filePath);
        if (journalRoot) {
            await this.ensureJournal(journalRoot);
        }
        const result = await this.callTool('entry_fix', { entry: filePath });
        // "renamed: <old_filename> → <new_filename>"
        const m = result.match(/^renamed: .+ → (.+)$/);
        if (m) {
            return path.join(path.dirname(filePath), m[1]);
        }
        return null;
    }

    /**
     * Create a blank entry (optionally under a parent) and return its absolute path.
     * Replaces `prepareNewEntry` from cli.ts.
     */
    async prepareNewEntry(cwd: string, parentId?: string): Promise<string> {
        await this.ensureJournal(cwd);
        const result = await this.callTool('entry_new', {
            parent: parentId ? { id: parentId } : undefined,
        });
        // "created: /absolute/path/to/entry.md"
        const m = result.match(/^created: (.+)$/);
        if (!m) { throw new Error(`Unexpected response from entry_new: ${result}`); }
        return m[1];
    }

    /** Resolve an entry ID (or prefix) to its absolute file path. */
    async resolvePath(entry: string, cwd: string): Promise<string> {
        await this.ensureJournal(cwd);
        const text = await this.callTool('entry_list', {});
        const entries: Array<{ id: string; path: string }> = JSON.parse(text);
        const match = entries.find(e => e.id === entry || e.id.startsWith(entry));
        if (!match) { throw new Error(`Entry not found: ${entry}`); }
        return match.path;
    }

    async removeEntry(entry: string, cwd: string): Promise<void> {
        await this.ensureJournal(cwd);
        await this.callTool('entry_remove', { entry });
    }

    /**
     * Set or clear an entry's parent.
     * Pass `undefined` as `parentId` to make the entry a root entry.
     */
    async setEntryParent(entryPath: string, parentId: string | undefined, cwd: string): Promise<void> {
        await this.ensureJournal(cwd);
        // null → UpdateOption::Clear (remove parent); @ID → UpdateOption::Set
        const parent = parentId !== undefined ? { id: parentId } : null;
        await this.callTool('entry_modify', { entry: entryPath, parent });
    }

    async listEntries(
        cwd: string,
        sortBy?: string,
        sortOrder?: string,
        period?: string,
    ): Promise<import('./cli').EntryRecord[]> {
        await this.ensureJournal(cwd);
        const text = await this.callTool('entry_list', {
            active: true,
            ...(sortBy    ? { sort_by: sortBy }       : {}),
            ...(sortOrder ? { sort_order: sortOrder } : {}),
            ...(period    ? { period }                : {}),
        });
        return JSON.parse(text);
    }

    async treeEntries(
        cwd: string,
        sortBy?: string,
        sortOrder?: string,
        period?: string,
    ): Promise<import('./cli').EntryRecord[]> {
        await this.ensureJournal(cwd);
        const text = await this.callTool('entry_tree', {
            active: true,
            ...(sortBy    ? { sort_by: sortBy }       : {}),
            ...(sortOrder ? { sort_order: sortOrder } : {}),
            ...(period    ? { period }                : {}),
        });
        return JSON.parse(text);
    }

    dispose(): void {
        this._client.close().catch(() => {});
    }
}
