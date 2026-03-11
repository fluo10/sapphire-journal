import * as path from 'path';
import * as vscode from 'vscode';
import { EntryRecord, listEntries } from './cli';
import { findJournalRoot } from './journal';

export class EntryItem extends vscode.TreeItem {
    constructor(public readonly record: EntryRecord) {
        super(record.title || '(untitled)', vscode.TreeItemCollapsibleState.None);

        this.command = {
            command: 'vscode.open',
            title: 'Open Entry',
            arguments: [vscode.Uri.file(record.path)],
        };

        if (record.task) {
            this.description = `[${record.task.status}]`;
        } else if (record.event) {
            this.description = record.event.start === record.event.end
                ? record.event.start.slice(0, 10)
                : `${record.event.start.slice(0, 10)} – ${record.event.end.slice(0, 10)}`;
        }

        const tagPart = record.tags.length > 0 ? `\nTags: #${record.tags.join(' #')}` : '';
        this.tooltip = `${record.id}${tagPart}`;
        this.contextValue = 'entry';
    }
}

export class EntryTreeProvider implements vscode.TreeDataProvider<EntryItem> {
    private _onDidChangeTreeData = new vscode.EventEmitter<EntryItem | undefined | void>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private _filter = '';

    get filter(): string { return this._filter; }

    refresh(): void {
        this._onDidChangeTreeData.fire();
    }

    setFilter(text: string): void {
        this._filter = text;
        this._onDidChangeTreeData.fire();
    }

    getTreeItem(element: EntryItem): vscode.TreeItem {
        return element;
    }

    async getChildren(): Promise<EntryItem[]> {
        const cwd = this._getCwd();
        if (!cwd) { return []; }

        let records: EntryRecord[];
        try {
            records = await listEntries(cwd);
        } catch {
            return [];
        }

        if (this._filter) {
            const f = this._filter.toLowerCase();
            records = records.filter(r =>
                r.title.toLowerCase().includes(f) ||
                r.id.toLowerCase().includes(f) ||
                r.tags.some(t => t.toLowerCase().includes(f))
            );
        }

        return records.map(r => new EntryItem(r));
    }

    private _getCwd(): string | null {
        const activeFile = vscode.window.activeTextEditor?.document.uri.fsPath;
        if (activeFile && findJournalRoot(activeFile)) {
            return path.dirname(activeFile);
        }
        return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
    }
}
