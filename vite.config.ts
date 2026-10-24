import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    watch: {
      // Rust 构建会替换、锁定二进制文件，不应触发前端监听。
      ignored: ['**/target/**'],
    },
  },
});
