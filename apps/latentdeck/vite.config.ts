import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";
import packageJson from "./package.json" with { type: "json" };

export default defineConfig({
  clearScreen: false,
  resolve: {
    conditions: ["browser"],
  },
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version),
  },
  plugins: [tailwindcss(), svelte()],
  test: {
    environment: "jsdom",
  },
});
