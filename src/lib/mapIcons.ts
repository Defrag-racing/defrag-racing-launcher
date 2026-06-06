// Map weapon / item / function icon mapping, ported from the web
// (defrag.racing Servers.vue). The icon SVGs are bundled with the launcher
// under public/images/{weapons,items,functions,powerups}/ (copied from the
// website), so the Maps grid doesn't fire a network request per icon per
// card - the icons load straight from the app bundle.
//
// The Map API returns weapons/items/functions as comma-separated strings
// (e.g. "rl,gl,lg"). splitCodes() turns one into a trimmed, de-duped list;
// each <kind>Icon()/<kind>Name() resolves a code to its icon URL + label.

// Bundled at the app root via Vite's public/ dir; empty base = local path.
const WEB = '';

/** Split a comma-separated weapons/items/functions string into codes. */
export const splitCodes = (s: string | null | undefined): string[] => {
    if (!s) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const raw of s.split(',')) {
        const c = raw.trim().toLowerCase();
        if (c && !seen.has(c)) { seen.add(c); out.push(c); }
    }
    return out;
};

const WEAPON_ICON: Record<string, string> = {
    gauntlet: '/images/weapons/iconw_gauntlet.svg',
    gt: '/images/weapons/iconw_gauntlet.svg',
    mg: '/images/weapons/iconw_machinegun.svg',
    sg: '/images/weapons/iconw_shotgun.svg',
    gl: '/images/weapons/iconw_grenade.svg',
    rl: '/images/weapons/iconw_rocket.svg',
    lg: '/images/weapons/iconw_lightning.svg',
    rg: '/images/weapons/iconw_railgun.svg',
    pg: '/images/weapons/iconw_plasma.svg',
    bfg: '/images/weapons/iconw_bfg.svg',
    grapple: '/images/weapons/iconw_grapple.svg',
    hook: '/images/weapons/iconw_grapple.svg',
    gh: '/images/weapons/iconw_grapple.svg',
};
const WEAPON_NAME: Record<string, string> = {
    gauntlet: 'Gauntlet', gt: 'Gauntlet', mg: 'Machine Gun', sg: 'Shotgun',
    gl: 'Grenade Launcher', rl: 'Rocket Launcher', lg: 'Lightning Gun',
    rg: 'Rail Gun', pg: 'Plasma Gun', bfg: 'BFG', grapple: 'Grappling Hook',
    hook: 'Grappling Hook', gh: 'Grappling Hook',
};

const ITEM_ICON: Record<string, string> = {
    enviro: '/images/powerups/envirosuit.svg',
    haste: '/images/powerups/haste.svg',
    quad: '/images/powerups/quad.svg',
    regen: '/images/powerups/regen.svg',
    invis: '/images/powerups/invis.svg',
    flight: '/images/powerups/flight.svg',
    health: '/images/items/iconh_yellow.svg',
    smallhealth: '/images/items/iconh_green.svg',
    bighealth: '/images/items/iconh_red.svg',
    mega: '/images/items/iconh_mega.svg',
    medkit: '/images/items/medkit.svg',
    shard: '/images/items/iconr_shard.svg',
    ya: '/images/items/iconr_yellow.svg',
    ra: '/images/items/iconr_red.svg',
    flag: '/images/items/iconf_blu2.svg',
};
const ITEM_NAME: Record<string, string> = {
    enviro: 'Battle Suit', haste: 'Haste', quad: 'Quad Damage',
    regen: 'Regeneration', invis: 'Invisibility', flight: 'Flight',
    health: 'Health (+25)', smallhealth: 'Small Health (+5)',
    bighealth: 'Large Health (+50)', mega: 'Mega Health (+100)',
    medkit: 'Medkit', shard: 'Armor Shard (+5)', ya: 'Yellow Armor (+50)',
    ra: 'Red Armor (+100)', flag: 'Flag',
};

const FUNCTION_ICON: Record<string, string> = {
    tele: '/images/functions/tele.svg',
    teleporter: '/images/functions/teleporter.svg',
    slick: '/images/functions/slick.svg',
    timer: '/images/functions/timer.svg',
    fog: '/images/functions/fog.svg',
    water: '/images/functions/water.svg',
    lava: '/images/functions/lava.svg',
    moving: '/images/functions/moving.svg',
    door: '/images/functions/door.svg',
    button: '/images/functions/button.svg',
    push: '/images/functions/push.svg',
    jumppad: '/images/functions/push.svg',
    launchramp: '/images/functions/push.svg',
    break: '/images/functions/break.svg',
    slime: '/images/functions/slime.svg',
    shootergl: '/images/functions/shootergl.svg',
    shooterpg: '/images/functions/shooterpg.svg',
    shooterrl: '/images/functions/shooterrl.svg',
};
const FUNCTION_NAME: Record<string, string> = {
    tele: 'Teleporter', teleporter: 'Teleporter', slick: 'Slick Surface',
    timer: 'Timer', fog: 'Fog', water: 'Water', lava: 'Lava',
    moving: 'Moving Platforms', door: 'Doors', button: 'Buttons',
    push: 'Push Trigger', jumppad: 'Jump Pad', launchramp: 'Launch Ramp',
    break: 'Breakable', slime: 'Slime', shootergl: 'Grenade Shooter',
    shooterpg: 'Plasma Shooter', shooterrl: 'Rocket Shooter',
};

const abs = (p: string) => `${WEB}${p}`;

export const weaponIcon = (c: string) => abs(WEAPON_ICON[c] ?? '/images/weapons/iconw_gauntlet.svg');
export const weaponName = (c: string) => WEAPON_NAME[c] ?? c.toUpperCase();
export const itemIcon = (c: string) => abs(ITEM_ICON[c] ?? '/images/items/iconh_yellow.svg');
export const itemName = (c: string) => ITEM_NAME[c] ?? c;
export const functionIcon = (c: string) => abs(FUNCTION_ICON[c] ?? '/images/functions/timer.svg');
export const functionName = (c: string) => FUNCTION_NAME[c] ?? c;
