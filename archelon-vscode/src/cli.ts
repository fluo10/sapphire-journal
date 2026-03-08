import { execFile } from 'child_process';
import { promisify } from 'util';
import * as fs from 'fs';
import * as path from 'path';
import * as vscode from 'vscode';

const execFileAsync = promisify(execFile);

let _extensionPath: string | undefined;

export function setExtensionPath(p: string) {
    _extensionPath = p;
}

function bin(): string {
    const configured = vscode.workspace.getConfiguration('archelon').get<string>('binaryPath', '');
    if (configured) { return configured; }

    if (_extensionPath) {
        const ext = process.platform === 'win32' ? '.exe' : '';
        const bundled = path.join(_extensionPath, 'bin', `archelon${ext}`);
        if (fs.existsSync(bundled)) { return bundled; }
    }

    return 'archelon';
}

/**
 * Run `archelon entry fix --touch <filePath>`.
 *
 * Returns the new absolute path if the file was renamed, or null if it stayed in place.
 * Throws on non-zero exit (e.g. not a managed entry, journal not found).
 */
export async function fixEntry(filePath: string): Promise<string | null> {
    const { stdout } = await execFileAsync(
        bin(),
        ['entry', 'fix', '--touch', filePath],
        { cwd: path.dirname(filePath) }
    );
    // "renamed: <old_filename> → <new_filename>"
    const m = stdout.trim().match(/^renamed: .+ → (.+)$/);
    if (m) {
        return path.join(path.dirname(filePath), m[1]);
    }
    return null;
}

/**
 * Run `archelon entry prepare` with the given working directory.
 *
 * Returns the absolute path of the newly created template file.
 * Throws on non-zero exit (e.g. journal not found).
 */
export async function prepareNewEntry(cwd: string): Promise<string> {
    const { stdout } = await execFileAsync(bin(), ['entry', 'path', '--new'], { cwd });
    return stdout.trim();
}

/**
 * Run `archelon entry path <entry>` and return the absolute file path.
 * Throws on non-zero exit (e.g. ID not found).
 */
export async function resolvePath(entry: string, cwd: string): Promise<string> {
    const { stdout } = await execFileAsync(bin(), ['entry', 'path', entry], { cwd });
    return stdout.trim();
}

/**
 * Run `archelon entry remove <entry>`.
 * Throws on non-zero exit.
 */
export async function removeEntry(entry: string, cwd: string): Promise<void> {
    await execFileAsync(bin(), ['entry', 'remove', entry], { cwd });
}

export interface EntryRecord {
    id: string;
    path: string;
    title: string;
    tags: string[];
    updated_at: string;
    task?: { status: string; due?: string; closed_at?: string } | null;
    event?: { start: string; end: string } | null;
}

/**
 * Run `archelon entry list --json` and return parsed records.
 * Throws on non-zero exit (e.g. journal not found).
 */
export async function listEntries(cwd: string): Promise<EntryRecord[]> {
    const { stdout } = await execFileAsync(bin(), ['entry', 'list', '--json'], { cwd });
    return JSON.parse(stdout) as EntryRecord[];
}
