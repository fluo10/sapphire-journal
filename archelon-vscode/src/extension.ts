import * as path from 'path';
import * as vscode from 'vscode';
import { bin, setExtensionPath, SortField, SortOrder } from './cli';
import { ArchelonMcpClient } from './mcp';
import { EntryItem, EntryTreeProvider } from './entryTreeProvider';
import { findJournalRoot, isManagedFilename } from './journal';

/** Return the workspace root, which is assumed to be the journal root. */
function getJournalCwd(): string | null {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
}

export async function activate(context: vscode.ExtensionContext) {
    setExtensionPath(context.extensionPath);
    vscode.commands.executeCommand('setContext', 'archelon.viewMode', 'tree');

    // ── MCP client ────────────────────────────────────────────────────────────
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    const mcp = new ArchelonMcpClient(bin(), workspaceRoot);
    context.subscriptions.push(mcp);
    try {
        await mcp.connect();
    } catch (err) {
        vscode.window.showErrorMessage(
            `Archelon: failed to start MCP server — ${err}. Check the "Archelon" output channel for details.`
        );
    }

    // ── Tree View: Entries ────────────────────────────────────────────────────
    const treeProvider = new EntryTreeProvider(mcp);
    const treeView = vscode.window.createTreeView('archelon.entries', {
        treeDataProvider: treeProvider,
        dragAndDropController: treeProvider,
        showCollapseAll: false,
    });
    context.subscriptions.push(treeView);

    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.refreshTree', () => {
            treeProvider.refresh();
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.sortTree', async () => {
            const fields: { label: string; field: SortField | undefined }[] = [
                { label: '$(circle-slash) Default (none)', field: undefined },
                { label: 'ID',           field: 'id' },
                { label: 'Title',        field: 'title' },
                { label: 'Updated at',   field: 'updated_at' },
                { label: 'Created at',   field: 'created_at' },
                { label: 'Task status',  field: 'task_status' },
                { label: 'Task due',     field: 'task_due' },
                { label: 'Event start',  field: 'event_start' },
            ];
            const picked = await vscode.window.showQuickPick(fields, {
                placeHolder: 'Sort entries by…',
            });
            if (picked === undefined) { return; }

            let order: SortOrder = 'asc';
            if (picked.field !== undefined) {
                const orderPick = await vscode.window.showQuickPick(
                    [{ label: '$(arrow-up) Ascending', value: 'asc' as SortOrder }, { label: '$(arrow-down) Descending', value: 'desc' as SortOrder }],
                    { placeHolder: 'Sort order' },
                );
                if (orderPick === undefined) { return; }
                order = orderPick.value;
            }

            treeProvider.setSort(picked.field, order);
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.filterTree', async () => {
            const current = treeProvider.filter;
            const input = await vscode.window.showInputBox({
                prompt: 'Filter entries by title, tag, or ID (leave empty to clear)',
                value: current,
                placeHolder: 'e.g. journal  or  #work',
            });
            if (input === undefined) { return; }
            treeProvider.setFilter(input);
            treeView.title = input ? `Entries: ${input}` : 'Entries';
        })
    );

    // ── Command: New Entry ────────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.newEntry', async () => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            // Create as sibling of the selected entry (same parent), or root if none selected
            const parentId = treeView.selection[0]?.parentId;
            try {
                const filePath = await mcp.prepareNewEntry(cwd, parentId);
                const doc = await vscode.workspace.openTextDocument(filePath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: failed to create entry — ${err}`);
            }
        })
    );

    // ── Command: New Child Entry ──────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.newChildEntry', async (item?: EntryItem) => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            const parentId = item?.record.id;
            try {
                const filePath = await mcp.prepareNewEntry(cwd, parentId);
                const doc = await vscode.workspace.openTextDocument(filePath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: failed to create child entry — ${err}`);
            }
        })
    );

    // ── Command: Open Entry ───────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.openEntry', async () => {
            const id = await vscode.window.showInputBox({
                prompt: 'Entry ID (or ID prefix)',
                placeHolder: '1a2b3c4',
            });
            if (!id) { return; }

            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            try {
                const filePath = await mcp.resolvePath(id, cwd);
                const doc = await vscode.workspace.openTextDocument(filePath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: entry not found — ${err}`);
            }
        })
    );

    // ── Command: Remove Entry ─────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.removeEntry', async (item?: EntryItem) => {
            let entryArg: string;
            let cwd: string;

            if (item) {
                // Invoked from tree context menu
                entryArg = item.record.path;
                cwd = path.dirname(entryArg);
            } else {
                // Default to the active file if it is a managed entry; otherwise ask for an ID.
                const activeFile = vscode.window.activeTextEditor?.document.uri.fsPath;
                let arg: string | undefined;
                if (activeFile && isManagedFilename(activeFile) && findJournalRoot(activeFile)) {
                    arg = activeFile;
                } else {
                    arg = await vscode.window.showInputBox({
                        prompt: 'Entry ID (or ID prefix) to remove',
                        placeHolder: '1a2b3c4',
                    });
                }
                if (!arg) { return; }
                entryArg = arg;
                cwd = getJournalCwd() ?? path.dirname(entryArg);
            }

            const label = path.basename(entryArg);
            const answer = await vscode.window.showWarningMessage(
                `Remove entry "${label}"? This cannot be undone.`,
                { modal: true },
                'Remove'
            );
            if (answer !== 'Remove') { return; }

            try {
                // Close open tabs for the file before deleting it.
                const targetPath = entryArg.includes(path.sep)
                    ? entryArg
                    : await mcp.resolvePath(entryArg, cwd);

                for (const group of vscode.window.tabGroups.all) {
                    for (const tab of group.tabs) {
                        if (tab.input instanceof vscode.TabInputText
                            && tab.input.uri.fsPath === targetPath) {
                            await vscode.window.tabGroups.close(tab);
                        }
                    }
                }

                await mcp.removeEntry(entryArg, cwd);
                treeProvider.refresh();
                vscode.window.showInformationMessage(`Archelon: removed ${label}`);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: remove failed — ${err}`);
            }
        })
    );

    // ── Command: Period Filter ────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.setPeriod', async () => {
            const presets: { label: string; period: string | undefined }[] = [
                { label: '$(list-unordered) All',        period: undefined },
                { label: '$(calendar) Yesterday',        period: 'yesterday' },
                { label: '$(calendar) Today',            period: 'today' },
                { label: '$(calendar) Tomorrow',         period: 'tomorrow' },
                { label: '$(calendar) Last week',         period: 'last_week' },
                { label: '$(calendar) This week',        period: 'this_week' },
                { label: '$(calendar) Next week',        period: 'next_week' },
                { label: '$(calendar) This month',       period: 'this_month' },
                { label: '$(calendar) Last month',       period: 'last_month' },
                { label: '$(calendar) Next month',       period: 'next_month' },
                { label: '$(edit) Custom date / range…', period: '__custom__' },
            ];
            const picked = await vscode.window.showQuickPick(presets, {
                placeHolder: 'Filter entries by period…',
            });
            if (picked === undefined) { return; }

            let period: string | undefined = picked.period;
            if (period === '__custom__') {
                const input = await vscode.window.showInputBox({
                    prompt: 'Date (YYYY-MM-DD) or range (YYYY-MM-DD,YYYY-MM-DD)',
                    placeHolder: '2026-03-12  or  2026-03-01,2026-03-31',
                    value: treeProvider.period ?? '',
                });
                if (input === undefined) { return; }
                period = input || undefined;
            }

            treeProvider.setPeriod(period);
            const label = period ? `Entries: ${period}` : 'Entries';
            treeView.title = treeProvider.filter ? `${label}: ${treeProvider.filter}` : label;
        })
    );

    // ── Commands: Toggle View Mode (tree ↔ list) ──────────────────────────────
    const switchViewMode = (mode: 'tree' | 'list') => {
        if (treeProvider.viewMode !== mode) {
            treeProvider.toggleViewMode();
        }
        vscode.commands.executeCommand('setContext', 'archelon.viewMode', mode);
    };
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.showListView', () => switchViewMode('list'))
    );
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.showTreeView', () => switchViewMode('tree'))
    );

    // ── Command: List Entries ─────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.listEntries', async () => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }

            let entries;
            try {
                entries = await mcp.listEntries(cwd);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: failed to list entries — ${err}`);
                return;
            }

            const items = entries.map(e => {
                // description: task status or event span
                let description = '';
                if (e.task) {
                    description = `[${e.task.status}]`;
                } else if (e.event) {
                    description = e.event.start === e.event.end
                        ? e.event.start.slice(0, 10)
                        : `${e.event.start.slice(0, 10)} – ${e.event.end.slice(0, 10)}`;
                }

                // detail: id · tags (if any)
                const tagPart = e.tags.length > 0 ? `  #${e.tags.join(' #')}` : '';
                const detail = `${e.id}${tagPart}`;

                return { label: e.title || '(untitled)', description, detail, entryPath: e.path };
            });

            const selected = await vscode.window.showQuickPick(items, {
                matchOnDescription: true,
                matchOnDetail: true,
                placeHolder: 'Select an entry to open',
            });
            if (!selected) { return; }

            try {
                const doc = await vscode.workspace.openTextDocument(selected.entryPath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: failed to open entry — ${err}`);
            }
        })
    );

    // ── Command: Init Journal ─────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.init', async () => {
            const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            try {
                await mcp.init(cwd);
                vscode.window.showInformationMessage('Archelon: journal initialized.');
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: init failed — ${err}`);
            }
        })
    );

    // ── Command: Cache Rebuild ────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.cacheRebuild', async () => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            try {
                await mcp.cacheRebuild(cwd);
                vscode.window.showInformationMessage('Archelon: cache rebuilt.');
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: cache rebuild failed — ${err}`);
            }
        })
    );

    // ── Command: Search Entries ───────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.searchEntries', async () => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }

            const qp = vscode.window.createQuickPick();
            qp.placeholder = 'Search entries…';
            qp.matchOnDescription = true;

            let debounce: ReturnType<typeof setTimeout> | undefined;

            qp.onDidChangeValue(value => {
                qp.busy = true;
                qp.items = [];
                if (debounce) { clearTimeout(debounce); }
                if (!value) { qp.busy = false; return; }

                debounce = setTimeout(async () => {
                    try {
                        const results = await mcp.searchEntries(cwd, value);
                        qp.items = results.map(r => ({
                            label: r.title || path.basename(r.path),
                            description: r.path,
                            entryPath: r.path,
                        }));
                    } catch {
                        qp.items = [];
                    } finally {
                        qp.busy = false;
                    }
                }, 200);
            });

            qp.onDidAccept(async () => {
                const selected = qp.selectedItems[0] as typeof qp.items[0] & { entryPath: string };
                qp.hide();
                if (!selected?.entryPath) { return; }
                try {
                    const doc = await vscode.workspace.openTextDocument(selected.entryPath);
                    await vscode.window.showTextDocument(doc);
                } catch (err) {
                    vscode.window.showErrorMessage(`Archelon: failed to open entry — ${err}`);
                }
            });

            qp.show();
        })
    );

    // ── On save: entry fix ─────────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.workspace.onDidSaveTextDocument(async (doc: vscode.TextDocument) => {
            const cfg = vscode.workspace.getConfiguration('archelon');
            if (!cfg.get<boolean>('autoFixOnSave', true)) {
                return;
            }

            const filePath = doc.uri.fsPath;
            if (!isManagedFilename(filePath)) {
                return;
            }
            if (!findJournalRoot(filePath)) {
                return;
            }

            try {
                const newPath = await mcp.fixEntry(filePath);
                treeProvider.refresh();
                if (newPath) {
                    // File was renamed: open new file and close old tabs.
                    const newDoc = await vscode.workspace.openTextDocument(newPath);
                    await vscode.window.showTextDocument(newDoc);

                    const oldUri = doc.uri;
                    for (const group of vscode.window.tabGroups.all) {
                        for (const tab of group.tabs) {
                            if (tab.input instanceof vscode.TabInputText
                                && tab.input.uri.fsPath === oldUri.fsPath) {
                                await vscode.window.tabGroups.close(tab);
                            }
                        }
                    }
                }
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: fix failed — ${err}`);
            }
        })
    );
}

export function deactivate() {}
