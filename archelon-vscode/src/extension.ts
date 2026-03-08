import * as path from 'path';
import * as vscode from 'vscode';
import { fixEntry, listEntries, prepareNewEntry, removeEntry, resolvePath } from './cli';
import { findJournalRoot, isManagedFilename } from './journal';

/** Return a cwd suitable for CLI calls: active file's dir if inside a journal, else workspace root. */
function getJournalCwd(): string | null {
    const activeFile = vscode.window.activeTextEditor?.document.uri.fsPath;
    if (activeFile && findJournalRoot(activeFile)) {
        return path.dirname(activeFile);
    }
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? null;
}

export function activate(context: vscode.ExtensionContext) {
    // ── Command: New Entry ────────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.newEntry', async () => {
            const cwd = getJournalCwd();
            if (!cwd) {
                vscode.window.showErrorMessage('Archelon: no workspace folder open.');
                return;
            }
            try {
                const filePath = await prepareNewEntry(cwd);
                const doc = await vscode.workspace.openTextDocument(filePath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: failed to create entry — ${err}`);
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
                const filePath = await resolvePath(id, cwd);
                const doc = await vscode.workspace.openTextDocument(filePath);
                await vscode.window.showTextDocument(doc);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: entry not found — ${err}`);
            }
        })
    );

    // ── Command: Remove Entry ─────────────────────────────────────────────────
    context.subscriptions.push(
        vscode.commands.registerCommand('archelon.removeEntry', async () => {
            // Default to the active file if it is a managed entry; otherwise ask for an ID.
            const activeFile = vscode.window.activeTextEditor?.document.uri.fsPath;
            let entryArg: string | undefined;

            if (activeFile && isManagedFilename(activeFile) && findJournalRoot(activeFile)) {
                entryArg = activeFile;
            } else {
                entryArg = await vscode.window.showInputBox({
                    prompt: 'Entry ID (or ID prefix) to remove',
                    placeHolder: '1a2b3c4',
                });
            }
            if (!entryArg) { return; }

            const label = path.basename(entryArg);
            const answer = await vscode.window.showWarningMessage(
                `Remove entry "${label}"? This cannot be undone.`,
                { modal: true },
                'Remove'
            );
            if (answer !== 'Remove') { return; }

            const cwd = getJournalCwd() ?? path.dirname(entryArg);
            try {
                // Close open tabs for the file before deleting it.
                const targetPath = entryArg.includes(path.sep)
                    ? entryArg
                    : await resolvePath(entryArg, cwd);

                for (const group of vscode.window.tabGroups.all) {
                    for (const tab of group.tabs) {
                        if (tab.input instanceof vscode.TabInputText
                            && tab.input.uri.fsPath === targetPath) {
                            await vscode.window.tabGroups.close(tab);
                        }
                    }
                }

                await removeEntry(entryArg, cwd);
                vscode.window.showInformationMessage(`Archelon: removed ${label}`);
            } catch (err) {
                vscode.window.showErrorMessage(`Archelon: remove failed — ${err}`);
            }
        })
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
                entries = await listEntries(cwd);
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

    // ── On save: entry fix --touch ────────────────────────────────────────────
    context.subscriptions.push(
        vscode.workspace.onDidSaveTextDocument(async (doc) => {
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
                const newPath = await fixEntry(filePath);
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
