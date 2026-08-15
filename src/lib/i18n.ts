// Translation for the launcher.
//
// The English sentence IS the key, the same convention the website uses. It
// costs a little duplication in the JSON files and buys two things worth more:
// a missing translation renders as correct English instead of `settings.demos.
// subfolders.label`, and a string in a `.vue` file reads as the sentence it
// will show rather than as a lookup nobody can picture.
//
// The files are the launcher's own. Sharing them with the website was
// considered and rejected: the two say different things in different places -
// the site explains what defrag.racing is, the launcher explains what your PC
// is about to do - and a shared file would have to serve both, which in
// practice means neither.
//
// One thing the launcher cannot translate: the system notifications the site
// sends. Those arrive as finished sentences the server has already assembled.
// Fixing that needs structured data on the wire, not a translation file here.

import { ref, computed } from 'vue';
import cs from '../locales/cs.json';
import de from '../locales/de.json';
import es from '../locales/es.json';
import fr from '../locales/fr.json';
import nl from '../locales/nl.json';
import pl from '../locales/pl.json';
import ru from '../locales/ru.json';
import sv from '../locales/sv.json';
import uk from '../locales/uk.json';

type Dictionary = Record<string, string>;

/** English has no file: it is the keys. */
const MESSAGES: Record<string, Dictionary> = { cs, de, es, fr, nl, pl, ru, sv, uk };

/** Offered in Settings, named in their own language - somebody looking for
 *  their language is not looking for the English word for it. */
export const LANGUAGES: { code: string; label: string }[] = [
    { code: 'en', label: 'English' },
    { code: 'cs', label: 'Čeština' },
    { code: 'de', label: 'Deutsch' },
    { code: 'es', label: 'Español' },
    { code: 'fr', label: 'Français' },
    { code: 'nl', label: 'Nederlands' },
    { code: 'pl', label: 'Polski' },
    { code: 'ru', label: 'Русский' },
    { code: 'sv', label: 'Svenska' },
    { code: 'uk', label: 'Українська' },
];

const SUPPORTED = new Set(LANGUAGES.map((l) => l.code));

export const locale = ref<string>('en');

/** What the app is running in right now, for the Settings picker. */
export const currentLanguage = computed(
    () => LANGUAGES.find((l) => l.code === locale.value) ?? LANGUAGES[0],
);

/**
 * Translate.
 *
 * Placeholders are `:name`, matching the website, and are replaced everywhere
 * they appear. Keep a linked or bolded word inside one placeholder rather than
 * splitting a sentence into fragments - a fragment cannot be reordered, and
 * every language this ships in reorders something.
 */
export const t = (key: string, params?: Record<string, string | number>): string => {
    const dict = MESSAGES[locale.value];
    let out = (dict && dict[key]) || key;
    if (params) {
        for (const [name, value] of Object.entries(params)) {
            out = out.split(`:${name}`).join(String(value));
        }
    }
    return out;
};

/**
 * Which language to start in.
 *
 * A saved choice wins outright. Otherwise the OS decides, because somebody
 * running a Czech Windows did not choose English, they just never opened
 * Settings - and a launcher that opens in a language you do not read is a
 * launcher whose Settings you cannot find. Region is dropped (`cs-CZ` is `cs`)
 * and anything unrecognised falls back to English.
 */
export const resolveLocale = (saved: string | null, os: string | null): string => {
    if (saved && SUPPORTED.has(saved)) return saved;
    const base = (os ?? '').split(/[-_]/)[0].toLowerCase();
    return SUPPORTED.has(base) ? base : 'en';
};

export const setLocale = (code: string) => {
    locale.value = SUPPORTED.has(code) ? code : 'en';
    // Screen readers and the browser's own hyphenation care; so does anything
    // that ever reads the page language later.
    document.documentElement.setAttribute('lang', locale.value);
};
