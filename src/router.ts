import { createRouter, createMemoryHistory } from 'vue-router';

// Memory history - we're a desktop app, not a webpage. No browser back
// button to reconcile with.
const router = createRouter({
    history: createMemoryHistory(),
    routes: [
        { path: '/', redirect: '/dashboard' },
        { path: '/onboarding', name: 'onboarding', component: () => import('./views/Onboarding.vue') },
        { path: '/dashboard', name: 'dashboard', component: () => import('./views/Dashboard.vue') },
        { path: '/servers', name: 'servers', component: () => import('./views/Servers.vue') },
        { path: '/records', name: 'records', component: () => import('./views/Records.vue') },
        { path: '/maps', name: 'maps', component: () => import('./views/Maps.vue') },
        // Library merged into the Demos (dashboard) view - keep the path
        // as a redirect so any lingering deep link / nav still lands somewhere.
        { path: '/library', redirect: '/dashboard' },
        { path: '/notifications', name: 'notifications', component: () => import('./views/Notifications.vue') },
        { path: '/history', name: 'history', component: () => import('./views/History.vue') },
        { path: '/settings', name: 'settings', component: () => import('./views/Settings.vue') },
        { path: '/version-mismatch', name: 'version-mismatch', component: () => import('./views/VersionMismatch.vue') },
    ],
});

export default router;
