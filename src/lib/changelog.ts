// Fetches and parses CHANGELOG.md from the launcher repo. The Dashboard's
// "What's new" panel uses this to show every version's notes between the
// user's installed build and the latest release.
//
// The repo's CHANGELOG.md is the single source of truth - we don't ship
// notes inside latest.json (which would balloon over time) and we don't
// hit the GitHub releases API (rate limit + auth surface). Raw GitHub
// fetches an immutable text file with a permissive CDN.

const RAW_URL = 'https://raw.githubusercontent.com/Defrag-racing/defrag-racing-launcher/main/CHANGELOG.md';

export interface ChangelogSection {
    version: string;
    body: string;
}

/** Lex `## X.Y.Z` headings out of the markdown into ordered sections. */
export const parseChangelog = (markdown: string): ChangelogSection[] => {
    const lines = markdown.split('\n');
    const sections: ChangelogSection[] = [];
    let current: ChangelogSection | null = null;
    const headingRe = /^##\s+(\d+\.\d+\.\d+)\s*$/;

    for (const line of lines) {
        const m = line.match(headingRe);
        if (m) {
            if (current) sections.push(current);
            current = { version: m[1], body: '' };
            continue;
        }
        if (current) {
            current.body += line + '\n';
        }
    }
    if (current) sections.push(current);
    return sections.map(s => ({ ...s, body: s.body.trim() }));
};

/** Semver-ish compare for `X.Y.Z` strings. Returns >0 if a > b. */
export const compareVersions = (a: string, b: string): number => {
    const pa = a.split('.').map(n => parseInt(n, 10) || 0);
    const pb = b.split('.').map(n => parseInt(n, 10) || 0);
    for (let i = 0; i < 3; i++) {
        if (pa[i] !== pb[i]) return pa[i] - pb[i];
    }
    return 0;
};

/** Fetch CHANGELOG.md and return only the sections strictly newer than
 *  `installed`. Latest first. Empty array means caller is already on the
 *  newest version or the changelog is unreachable. */
export const fetchChangelogSince = async (installed: string): Promise<ChangelogSection[]> => {
    const resp = await fetch(RAW_URL, { cache: 'no-store' });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const md = await resp.text();
    const sections = parseChangelog(md);
    return sections
        .filter(s => compareVersions(s.version, installed) > 0)
        .sort((a, b) => compareVersions(b.version, a.version));
};

/** Tiny markdown renderer for the subset we use in CHANGELOG.md:
 *  paragraphs, bullet lists (- ...), bold (**...**), italics (*...*),
 *  inline code (`...`), and links [text](url). No HTML smuggling - we
 *  HTML-escape input first and only re-introduce a vetted set of tags. */
export const renderMarkdown = (md: string): string => {
    const esc = (s: string) =>
        s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

    const inline = (s: string) =>
        esc(s)
            .replace(/`([^`]+)`/g, '<code class="bg-white/10 px-1 rounded text-[0.85em]">$1</code>')
            .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
            .replace(/(^|\s)\*([^*]+)\*/g, '$1<em>$2</em>')
            .replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, t, u) => {
                const url = /^https?:\/\//.test(u) ? u : '#';
                return `<a href="${url}" target="_blank" rel="noopener" class="text-brand-300 hover:underline">${t}</a>`;
            });

    const lines = md.split('\n');
    const out: string[] = [];
    let inList = false;
    let para: string[] = [];

    const flushPara = () => {
        if (para.length === 0) return;
        out.push(`<p class="my-2 leading-relaxed">${inline(para.join(' '))}</p>`);
        para = [];
    };

    for (const raw of lines) {
        const line = raw.trimEnd();
        if (/^-\s+/.test(line)) {
            flushPara();
            if (!inList) { out.push('<ul class="my-2 space-y-1 list-disc pl-5">'); inList = true; }
            out.push(`<li>${inline(line.replace(/^-\s+/, ''))}</li>`);
        } else if (line === '') {
            if (inList) { out.push('</ul>'); inList = false; }
            flushPara();
        } else {
            if (inList) { out.push('</ul>'); inList = false; }
            para.push(line);
        }
    }
    if (inList) out.push('</ul>');
    flushPara();
    return out.join('');
};
