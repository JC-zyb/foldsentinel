import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    watch: {
      // 构建产物和隔离测试的 WebView2 资料可能被锁定，不应触发前端监听。
      ignored: ['**/target/**', '**/work/**'],
    },
  },
});
