import * as path from 'path';
import * as vscode from 'vscode';
import { EntryRecord, SortField, SortOrder, listEntries, treeEntries } from './cli';
import { findJournalRoot } from './journal';

export type ViewMode = 'tree' | 'list';

export class EntryItem extends vscode.TreeItem {
    constructor(
        public readonly record: EntryRecord,
        public readonly children: EntryRecord[],
    ) {
        const typeSymbol = record.symbols?.[record.symbols.length - 1]?.emoji ?? '📝';
        const freshnessSymbol = record.symbols && record.symbols.length > 1 ? record.symbols[0].emoji : '　';
        const emojiSlot = `${freshnessSymbol}${typeSymbol}`;
        super(
            `${emojiSlot} ${record.title || '(untitled)'}`,
            children.length > 0
                ? vscode.TreeItemCollapsibleState.Expanded
                : vscode.TreeItemCollapsibleState.None,
        );

        this.command = {
            command: 'vscode.open',
            title: 'Open Entry',
            arguments: [vscode.Uri.file(record.path)],
        };

        let desc = `@${record.id}`;
        if (record.event) {
            const span = record.event.start === record.event.end
                ? record.event.start.slice(0, 10)
                : `${record.event.start.slice(0, 10)} – ${record.event.end.slice(0, 10)}`;
            desc += ` ${span}`;
        }
        this.description = desc;

        const md = new vscode.MarkdownString();
        md.appendMarkdown(`**ID:** \`${record.id}\`  \n`);
        if (record.tags.length > 0) {
            md.appendMarkdown(`**Tags:** ${record.tags.map(t => `\`#${t}\``).join(' ')}  \n`);
        }
        md.appendMarkdown(`**Updated:** ${record.updated_at.slice(0, 19).replace('T', ' ')}  \n`);
        if (record.task) {
            let taskLine = `**Task:** ${record.task.status}`;
            if (record.task.due) { taskLine += ` (due: ${record.task.due.slice(0, 10)})`; }
            if (record.task.closed_at) { taskLine += ` (closed: ${record.task.closed_at.slice(0, 10)})`; }
            md.appendMarkdown(taskLine + `  \n`);
        }
        if (record.event) {
            const span = record.event.start === record.event.end
                ? record.event.start.slice(0, 10)
                : `${record.event.start.slice(0, 10)} – ${record.event.end.slice(0, 10)}`;
            md.appendMarkdown(`**Event:** ${span}  \n`);
        }
        this.tooltip = md;
        this.contextValue = 'entry';
    }
}

export class EntryTreeProvider implements vscode.TreeDataProvider<EntryItem> {
    private _onDidChangeTreeData = new vscode.EventEmitter<EntryItem | undefined | void>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private _filter = '';
    private _sortBy: SortField | undefined = undefined;
    private _sortOrder: SortOrder = 'asc';
    private _viewMode: ViewMode = 'tree';
    private _period: string | undefined = undefined;
    private _rootRecords: EntryRecord[] = [];

    get filter(): string { return this._filter; }
    get sortBy(): SortField | undefined { return this._sortBy; }
    get sortOrder(): SortOrder { return this._sortOrder; }
    get viewMode(): ViewMode { return this._viewMode; }
    get period(): string | undefined { return this._period; }

    refresh(): void {
        this._rootRecords = [];
        this._onDidChangeTreeData.fire();
    }

    setFilter(text: string): void {
        this._filter = text;
        this._rootRecords = [];
        this._onDidChangeTreeData.fire();
    }

    setSort(sortBy: SortField | undefined, sortOrder: SortOrder): void {
        this._sortBy = sortBy;
        this._sortOrder = sortOrder;
        this._rootRecords = [];
        this._onDidChangeTreeData.fire();
    }

    setPeriod(period: string | undefined): void {
        this._period = period;
        this._rootRecords = [];
        this._onDidChangeTreeData.fire();
    }

    toggleViewMode(): ViewMode {
        this._viewMode = this._viewMode === 'tree' ? 'list' : 'tree';
        this._rootRecords = [];
        this._onDidChangeTreeData.fire();
        return this._viewMode;
    }

    getTreeItem(element: EntryItem): vscode.TreeItem {
        return element;
    }

    async getChildren(element?: EntryItem): Promise<EntryItem[]> {
        // In list mode, all entries are top-level (no children)
        if (element) {
            return this._viewMode === 'tree' ? this._toItems(element.children) : [];
        }

        const cwd = this._getCwd();
        if (!cwd) { return []; }

        try {
            if (this._viewMode === 'list') {
                this._rootRecords = await listEntries(cwd, this._sortBy, this._sortOrder, this._period);
            } else {
                this._rootRecords = await treeEntries(cwd, this._sortBy, this._sortOrder, this._period);
            }
        } catch {
            return [];
        }

        let roots = this._rootRecords;
        if (this._filter) {
            roots = this._filterRecords(roots, this._filter.toLowerCase());
        }

        return this._toItems(roots);
    }

    private _toItems(records: EntryRecord[]): EntryItem[] {
        return records.map(r => new EntryItem(r, r.children ?? []));
    }

    /** Recursively keep records whose title/id/tags match, preserving matched subtrees. */
    private _filterRecords(records: EntryRecord[], f: string): EntryRecord[] {
        const result: EntryRecord[] = [];
        for (const r of records) {
            const selfMatch =
                r.title.toLowerCase().includes(f) ||
                r.id.toLowerCase().includes(f) ||
                r.tags.some(t => t.toLowerCase().includes(f));
            const filteredChildren = this._filterRecords(r.children ?? [], f);
            if (selfMatch || filteredChildren.length > 0) {
                result.push({ ...r, children: filteredChildren });
            }
        }
        return result;
    }

    private _getCwd(): string | null {
        const activeFile = vscode.window.activeTextEditor?.document.uri.fsPath;
        if (activeFile && findJournalRoot(activeFile)) {
            return path.dirname(activeFile);
        }
        return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
    }
}
