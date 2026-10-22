import js from '@eslint/js';
import ts from 'typescript-eslint';
export default ts.config({ ignores: ['dist/**', 'node_modules/**', 'target/**', 'engine/target/**', 'src-tauri/gen/**', 'test-results/**', 'playwright-report/**', 'release/**', '.toolchains/**'] }, js.configs.recommended,
  ...ts.configs.recommended, { rules: { '@typescript-eslint/no-explicit-any': 'error' } });
