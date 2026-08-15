import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import router from './router';
import './style.css';
import { t } from './lib/i18n';

const app = createApp(App);
// `$t` in templates, `t` imported directly in script blocks. Global rather
// than per-component because there is no view that does not need it.
app.config.globalProperties.$t = t;
app.use(createPinia()).use(router).mount('#app');
