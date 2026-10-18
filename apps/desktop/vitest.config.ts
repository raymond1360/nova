import { defineConfig, mergeConfig } from 'vitest/config'
import viteConfig from './vite.config'

export default mergeConfig(viteConfig, defineConfig({
  test: {
    // Compile real SFC templates for the custom Vue host renderer, without a DOM dependency.
    testTransformMode: { web: ['**/*.interaction.test.ts'] },
  },
}))
