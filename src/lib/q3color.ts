// Quake 3 / Defrag colored-name renderer. Mirrors q3tohtml() in
// resources/js/app.js + the .q3c-* CSS palette in resources/css/items.css
// on the defrag-racing-project web app. Any palette change on the web
// should land here too (and vice-versa) so launcher names match
// pixel-for-pixel what the user sees in their browser.
//
// Inline styles instead of CSS classes so the helper is self-contained:
// callers just v-html the output without remembering a separate
// stylesheet import at every site.

const Q3_PALETTE: Record<string, string> = {
    '0': 'black',
    '1': 'rgb(255, 41, 41)',
    '2': 'green',
    '3': 'yellow',
    '4': 'rgb(37, 106, 255)',
    '5': 'cyan',
    '6': 'magenta',
    '7': 'white',
    '8': 'rgb(255, 111, 27)',
    '9': 'gray',
    a: 'rgb(255, 34, 26)',
    b: 'rgb(255, 81, 0)',
    c: 'rgb(255, 123, 0)',
    d: 'rgb(255, 153, 0)',
    e: 'rgb(255, 217, 0)',
    f: 'rgb(179, 255, 0)',
    g: 'rgb(115, 255, 0)',
    h: 'rgb(60, 255, 0)',
    i: 'rgb(9, 255, 0)',
    j: 'rgb(0, 255, 115)',
    k: 'rgb(0, 255, 149)',
    l: 'rgb(0, 255, 213)',
    m: 'rgb(0, 255, 242)',
    n: 'rgb(0, 174, 255)',
    o: 'rgb(0, 132, 255)',
    p: 'rgb(0, 110, 255)',
    q: 'rgb(4, 0, 255)',
    r: 'rgb(102, 0, 219)',
    s: 'rgb(168, 0, 219)',
    t: 'rgb(204, 0, 255)',
    u: 'rgb(255, 52, 245)',
    v: 'rgb(255, 0, 191)',
    w: 'rgb(255, 0, 128)',
    x: 'rgb(255, 0, 13)',
    y: 'rgb(255, 255, 255)',
    z: 'gray',
};

const escapeHtml = (s: string): string =>
    s.replace(/&/g, '&amp;')
     .replace(/</g, '&lt;')
     .replace(/>/g, '&gt;')
     .replace(/"/g, '&quot;');

/** Convert a Quake 3 colored string ("^1Hello ^7world") to HTML with
 *  inline-styled <span> wrappers. Mirrors the web's q3tohtml() byte-for-
 *  byte: default color is white (^7), case-sensitive (^a != ^A), and a
 *  trailing unmatched ^ at end of string is silently dropped. Use with
 *  v-html. */
export function q3ToHtml(name: string | null | undefined): string {
    if (!name) return '';
    let result = '';
    let color = '7';
    let buffer = '';
    const flush = () => {
        if (buffer) {
            const c = Q3_PALETTE[color] ?? Q3_PALETTE['7'];
            result += `<span style="color:${c}">${escapeHtml(buffer)}</span>`;
            buffer = '';
        }
    };
    for (let i = 0; i < name.length; i++) {
        if (name[i] === '^') {
            if (name[i + 1] === '^') {
                buffer += '^';
            } else if (i + 1 < name.length) {
                flush();
                color = name[i + 1];
                i++;
            }
        } else {
            buffer += name[i];
        }
    }
    flush();
    return result;
}
