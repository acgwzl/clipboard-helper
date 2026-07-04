// 字体本地打包(@fontsource):离线/无 CDN 也保证渲染指定字体,不回退宋体
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-sans/700.css";
import "@fontsource/zilla-slab/600.css";
import "@fontsource/zilla-slab/700.css";
import "@fontsource/courier-prime/400.css";
import "@fontsource/courier-prime/700.css";
import "@fontsource/noto-serif-sc/700.css";

import { createApp } from "vue";
import App from "./App.vue";

createApp(App).mount("#app");
