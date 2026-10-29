import DefaultTheme from 'vitepress/theme';
import { h } from 'vue';
import { useData } from 'vitepress';
import './style.css';

export default {
  extends: DefaultTheme,
  Layout() {
    const { theme } = useData();
    return h(DefaultTheme.Layout, null, {
      'doc-before': () => h('p', { class: 'docs-release', role: 'note' }, theme.value.docsLabel),
    });
  },
};
