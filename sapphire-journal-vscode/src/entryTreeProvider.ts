import * as vscode from 'vscode';
import { EntryRecord, SortField, SortOrder } from './cli';
import { ArchelonMcpClient } from './mcp';

export type ViewMode = 'tree' | 'list';

function typeIconId(flag: string): string {
    switch (flag) {
        case 'event':        return 'calendar';
        case 'event_closed': return 'window-active';
        case 'done':         return 'pass';
        case 'cancelled':    return 'skip';
        case 'in_progress':  return 'play-circle';
        case 'archived':     return 'archive';
        case 'open':         return 'circle-large';
        default:             return 'note';
    }
}

export class EntryItem extends vscode.TreeItem {
    constructor(
        public readonly record: EntryRecord,
        public readonly children: EntryRecord[],
        public readonly parentId: string | undefined = undefined,
    ) {
        super(
            record.title || '(untitled)',
            children.length > 0
                ? vscode.TreeItemCollapsibleState.Expanded
                : vscode.TreeItemCollapsibleState.None,
        );

        const typeFlag = record.flags?.[record.flags.length - 1] ?? 'note';
        this.iconPath = new vscode.ThemeIcon(typeIconId(typeFlag));

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
        if (record.flags && record.flags.length > 1) {
            md.appendMarkdown(`**Freshness:** ${record.flags[0]}  \n`);
        }
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

const ENTRY_MIME_TYPE = 'application/vnd.code.tree.sapphire-journal.entries';

export class EntryTreeProvider implements vscode.TreeDataProvider<EntryItem>, vscode.TreeDragAndDropController<EntryItem> {
    readonly dragMimeTypes = [ENTRY_MIME_TYPE];
    readonly dropMimeTypes = [ENTRY_MIME_TYPE];
    private _onDidChangeTreeData = new vscode.EventEmitter<EntryItem | undefined | void>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private _filter = '';
    private _sortBy: SortField | undefined;
    private _sortOrder: SortOrder;
    private _viewMode: ViewMode = 'tree';
    private _period: string | undefined;
    private _rootRecords: EntryRecord[] = [];

    constructor(private readonly _mcp: ArchelonMcpClient) {
        const cfg = vscode.workspace.getConfiguration('sapphire-journal');
        const rawPeriod = cfg.get<string>('defaultPeriod', 'today');
        this._period = rawPeriod === '' ? undefined : rawPeriod;
        const rawSortField = cfg.get<string>('defaultSortField', 'updated_at');
        this._sortBy = rawSortField === '' ? undefined : rawSortField as SortField;
        const rawSortOrder = cfg.get<string>('defaultSortOrder', 'desc');
        this._sortOrder = rawSortOrder === 'desc' ? 'desc' : 'asc';
    }

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
            return this._viewMode === 'tree' ? this._toItems(element.children, element.record.id) : [];
        }

        const cwd = this._getCwd();
        if (!cwd) { return []; }

        try {
            if (this._viewMode === 'list') {
                this._rootRecords = await this._mcp.listEntries(cwd, this._sortBy, this._sortOrder, this._period);
            } else {
                this._rootRecords = await this._mcp.treeEntries(cwd, this._sortBy, this._sortOrder, this._period);
            }
        } catch (err) {
            vscode.window.showErrorMessage(`Archelon: failed to load entries — ${err}`);
            return [];
        }

        let roots = this._rootRecords;
        if (this._filter) {
            roots = this._filterRecords(roots, this._filter.toLowerCase());
        }

        return this._toItems(roots);
    }

    handleDrag(source: readonly EntryItem[], dataTransfer: vscode.DataTransfer): void {
        dataTransfer.set(ENTRY_MIME_TYPE, new vscode.DataTransferItem(
            source.map(s => ({ id: s.record.id, path: s.record.path }))
        ));
    }

    async handleDrop(target: EntryItem | undefined, dataTransfer: vscode.DataTransfer): Promise<void> {
        const item = dataTransfer.get(ENTRY_MIME_TYPE);
        if (!item) { return; }
        const sources: { id: string; path: string }[] = item.value;

        const cwd = this._getCwd();
        if (!cwd) { return; }

        // target === undefined means dropped onto the tree root → unset parent
        const targetId = target?.record.id;

        const errors: string[] = [];
        for (const src of sources) {
            if (src.id === targetId) { continue; }
            try {
                await this._mcp.setEntryParent(src.path, targetId, cwd);
            } catch (err) {
                errors.push(`@${src.id}: ${err}`);
            }
        }
        if (errors.length > 0) {
            vscode.window.showErrorMessage(`Archelon: failed to reparent — ${errors.join(', ')}`);
        }
        this.refresh();
    }

    private _toItems(records: EntryRecord[], parentId?: string): EntryItem[] {
        return records.map(r => new EntryItem(r, r.children ?? [], parentId));
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
        return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
    }
}
