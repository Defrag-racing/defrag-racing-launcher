import { createRouter, createMemoryHistory } from 'vue-router';

// Memory history — we're a desktop app, not a webpage. No browser back
// button to reconcile with.
const router = createRouter({
    history: createMemoryHistory(),
    routes: [
        { path: '/', redirect: '/dashboard' },
        { path: '/onboarding', name: 'onboarding', component: () => import('./views/Onboarding.vue') },
        { path: '/dashboard', name: 'dashboard', component: () => import('./views/Dashboard.vue') },
        { path: '/settings', name: 'settings', component: () => import('./views/Settings.vue') },
    ],
});

export default router;
