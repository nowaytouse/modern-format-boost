import eslint from '@eslint/js';
import tseslint from 'typescript-eslint';
import pluginVue from 'eslint-plugin-vue';
import unicorn from 'eslint-plugin-unicorn';
import sonarjs from 'eslint-plugin-sonarjs';
import globals from 'globals';

export default tseslint.config(
  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...pluginVue.configs['flat/recommended'],
  unicorn.configs['flat/recommended'],
  sonarjs.configs.recommended,
  {
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      globals: {
        ...globals.browser,
        ...globals.es2021,
        ...globals.node,
      },
      parserOptions: {
        parser: tseslint.parser,
        project: ['./tsconfig.json', './tsconfig.node.json'],
        extraFileExtensions: ['.vue'],
      },
    },
    rules: {
      // Core Vue rules
      'vue/multi-word-component-names': 'off',

      // Unicorn is extremely opinionated, we relax a few that clash with standard Vue/Tauri practices
      'unicorn/filename-case': [
        'error',
        {
          cases: {
            camelCase: true,
            pascalCase: true, // For Vue components
          },
        },
      ],
      'unicorn/prevent-abbreviations': 'off',
      'unicorn/no-null': 'off', // null is often used in Vue refs
      'unicorn/no-top-level-assignment-in-function': 'off', // Script setup top level vars are instance scoped
      'unicorn/no-unnecessary-global-this': 'off',
      'unicorn/prefer-await': 'off',
      'unicorn/name-replacements': 'off',
      'sonarjs/pseudo-random': 'off',
      'sonarjs/deprecation': 'off',
      
      // We will turn these strict TS rules into 'error' and fix them.
      '@typescript-eslint/no-explicit-any': 'error',
      '@typescript-eslint/no-unsafe-assignment': 'error',
      '@typescript-eslint/no-unsafe-member-access': 'error',
      '@typescript-eslint/no-unsafe-call': 'error',
      '@typescript-eslint/no-unsafe-argument': 'error',
      '@typescript-eslint/no-unsafe-return': 'error',
    },
  },
  {
    ignores: ['dist/', 'src-tauri/', 'node_modules/', '*.cjs', '*.config.ts', '*.config.js', '*.js', 'src/**/*.js'],
  }
);
