import js from '@eslint/js';
import ts from 'typescript-eslint';
export default ts.config({ ignores: ['dist/**', 'node_modules/**', 'engine/target/**', 'test-results/**', 'playwright-report/**', 'release/**', '.toolchains/**', 'native/**/build/**', 'native/**/target/**', 'native/**/.gradle/**'] }, js.configs.recommended,
  { files: ['desktop/**/*.cjs'], languageOptions: { globals: { require: 'readonly', __dirname: 'readonly', process: 'readonly', setTimeout: 'readonly', clearTimeout: 'readonly', URL: 'readonly', fetch: 'readonly' } } },
  { files: ['public/phone-sw.js'], languageOptions: { globals: { self: 'readonly' } } },
  ...ts.configs.recommended, { files: ['desktop/**/*.cjs'], rules: { '@typescript-eslint/no-require-imports': 'off' } }, { rules: { '@typescript-eslint/no-explicit-any': 'error' } });
