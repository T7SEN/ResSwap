import js from '@eslint/js';
import type { Linter } from 'eslint';
import { defineConfig, globalIgnores } from 'eslint/config';
import prettier from 'eslint-config-prettier/flat';
// @ts-expect-error TS7016: eslint-plugin-jsx-a11y 6.10.2 ships no type declarations.
import jsxA11yUntyped from 'eslint-plugin-jsx-a11y';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import globals from 'globals';
import tseslint from 'typescript-eslint';

// The one part of the plugin this file uses, typed here because the package has no types.
const jsxA11y = jsxA11yUntyped as { flatConfigs: { recommended: Linter.Config } };

const apiOutsideCore = {
  group: ['@tauri-apps/api', '@tauri-apps/api/*'],
  message: 'Only src/core/ talks to the Rust core (REQ-UI-15).',
};
const pluginPackages = {
  group: ['@tauri-apps/plugin-*'],
  message: 'The web view has no plugin permissions (D-003).',
};
const baseUiOutsideUi = {
  group: ['@base-ui/react', '@base-ui/react/*'],
  message: 'Only src/components/ui/ imports Base UI.',
};
const invokeOutsideClient = {
  name: '@tauri-apps/api/core',
  importNames: ['invoke'],
  message: 'Only src/core/client.ts calls invoke.',
};
const mocksOutsideMock = {
  name: '@tauri-apps/api/mocks',
  message: 'Mocks live in src/core/mock/ only.',
};

export default defineConfig([
  globalIgnores([
    'dist',
    'dist-mock',
    'src-tauri',
    'src/core/types',
    'docs/results',
    'coverage',
    'playwright-report',
    'test-results',
  ]),
  {
    files: ['**/*.{js,mjs,cjs}'],
    extends: [js.configs.recommended],
    languageOptions: { globals: globals.node },
  },
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.strictTypeChecked,
      tseslint.configs.stylisticTypeChecked,
    ],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    rules: {
      eqeqeq: 'error',
      '@typescript-eslint/switch-exhaustiveness-check': 'error',
      // Data shapes and unions are type aliases, as the ts-rs output is.
      '@typescript-eslint/consistent-type-definitions': ['error', 'type'],
    },
  },
  {
    files: ['src/**/*.{ts,tsx}'],
    extends: [reactHooks.configs.flat.recommended, jsxA11y.flatConfigs.recommended],
    languageOptions: { globals: globals.browser },
    rules: {
      'no-console': 'error',
      'no-restricted-syntax': [
        'error',
        { selector: 'ExportDefaultDeclaration', message: 'Use a named export.' },
      ],
      'no-restricted-imports': [
        'error',
        { patterns: [apiOutsideCore, pluginPackages, baseUiOutsideUi] },
      ],
    },
  },
  { files: ['src/**/*.tsx'], extends: [reactRefresh.configs.vite] },
  {
    files: ['src/components/ui/**'],
    rules: {
      'no-restricted-imports': ['error', { patterns: [apiOutsideCore, pluginPackages] }],
    },
  },
  {
    files: ['src/core/**'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [invokeOutsideClient, mocksOutsideMock],
          patterns: [pluginPackages, baseUiOutsideUi],
        },
      ],
    },
  },
  {
    files: ['src/core/client.ts'],
    rules: {
      'no-restricted-imports': [
        'error',
        { paths: [mocksOutsideMock], patterns: [pluginPackages, baseUiOutsideUi] },
      ],
    },
  },
  {
    files: ['src/core/mock/**'],
    rules: {
      'no-restricted-imports': [
        'error',
        { paths: [invokeOutsideClient], patterns: [pluginPackages, baseUiOutsideUi] },
      ],
    },
  },
  {
    // The facade's own tests drive it through mockIPC, as the mocked core does.
    files: ['src/core/**/*.test.{ts,tsx}'],
    rules: {
      'no-restricted-imports': [
        'error',
        { paths: [invokeOutsideClient], patterns: [pluginPackages, baseUiOutsideUi] },
      ],
    },
  },
  {
    // Tests may assert that a value they just checked is present.
    files: ['src/**/*.test.{ts,tsx}', 'e2e/**/*.ts'],
    rules: { '@typescript-eslint/no-non-null-assertion': 'off' },
  },
  {
    files: ['e2e/**/*.ts', '*.config.ts', 'vitest.setup.ts'],
    languageOptions: { globals: globals.node },
  },
  prettier,
]);
