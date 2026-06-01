// Path display helpers.
//
// The watcher stores the Windows *extended-length* form of the demos
// folder (e.g. `\\?\E:\GamesLibraries\...\demos`). That `\\?\` prefix is
// a Win32 API detail that lets paths exceed MAX_PATH - it should never
// be shown to a human. These helpers strip it for display and produce a
// short "tail" snippet so the UI can show `…\defrag\demos` instead of a
// 90-character absolute path.

/** Strip the Windows extended-length (`\\?\` / `\\?\UNC\`) prefix so the
 *  path reads the way the user typed it: `\\?\E:\foo` -> `E:\foo`,
 *  `\\?\UNC\srv\share` -> `\\srv\share`. Non-Windows / already-clean
 *  paths pass through unchanged. */
export function displayPath(p?: string | null): string {
    if (!p) return '';
    if (p.startsWith('\\\\?\\UNC\\')) return '\\\\' + p.slice(8);
    if (p.startsWith('\\\\?\\')) return p.slice(4);
    return p;
}

/** Last `segments` path components, prefixed with `…\` when the path is
 *  longer than that. `E:\a\b\c\defrag\demos` -> `…\defrag\demos`. Keeps
 *  the native separator of the (cleaned) path so Windows shows `\` and
 *  POSIX shows `/`. */
export function folderSnippet(p?: string | null, segments = 2): string {
    const clean = displayPath(p);
    if (!clean) return '';
    const sep = clean.includes('\\') ? '\\' : '/';
    const parts = clean.split(/[\\/]/).filter(Boolean);
    if (parts.length <= segments) return clean;
    return '…' + sep + parts.slice(-segments).join(sep);
}

/** Just the leaf folder name: `…\defrag\demos` -> `demos`. */
export function folderName(p?: string | null): string {
    const clean = displayPath(p);
    if (!clean) return '';
    const parts = clean.split(/[\\/]/).filter(Boolean);
    return parts.length ? parts[parts.length - 1] : clean;
}
