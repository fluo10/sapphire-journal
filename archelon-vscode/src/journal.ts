import * as fs from 'fs';
import * as path from 'path';

/**
 * Walk up the directory tree from `filePath` looking for a `.archelon/` directory.
 * Returns the journal root path, or null if not found.
 */
export function findJournalRoot(filePath: string): string | null {
    let dir = path.dirname(filePath);
    while (true) {
        if (fs.existsSync(path.join(dir, '.archelon'))) {
            return dir;
        }
        const parent = path.dirname(dir);
        if (parent === dir) {
            return null;
        }
        dir = parent;
    }
}

/**
 * Returns true if the filename looks like an archelon-managed entry:
 * 7 alphanumeric characters followed by `_` or `.`, ending in `.md`.
 *
 * Examples: `1a2b3c4_my-note.md`, `1a2b3c4.md`
 */
export function isManagedFilename(filePath: string): boolean {
    const name = path.basename(filePath);
    return /^[0-9a-zA-Z]{7}[_.]/.test(name) && name.endsWith('.md');
}
