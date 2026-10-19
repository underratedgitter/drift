import { execFileSync } from 'node:child_process';
import { accessSync, constants, readdirSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import process from 'node:process';
import console from 'node:console';

const directory = resolve('engine/target/release/deps');
const candidates = readdirSync(directory).filter(name => /^integration-[a-f0-9]+(?:\.exe)?$/.test(name))
  .map(name => resolve(directory, name)).filter(path => statSync(path).isFile())
  .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
const binary = candidates[0];
if (!binary) throw new Error('Build the release integration tests first; see PERFORMANCE.md.');
accessSync(binary, constants.X_OK);
const count = Number(process.env.DRIFT_BENCH_RUNS ?? 5);
if (!Number.isInteger(count) || count < 1 || count > 20) throw new Error('DRIFT_BENCH_RUNS must be 1–20.');
const runs = [];
for (let index = 0; index < count; index++) {
  const args = ['benchmark_streaming', '--ignored', '--nocapture', '--test-threads=1'];
  const result = execFileSync(binary, args, { encoding: 'utf8' });
  const match = result.match(/BENCHMARK: (\d+) bytes, ([\d.]+) seconds, ([\d.]+) MiB\/s/);
  if (!match) throw new Error('Benchmark output was not recognized.');
  const run = { bytes: Number(match[1]), seconds: Number(match[2]), mebibytesPerSecond: Number(match[3]) };
  runs.push(run);
  console.log(`Run ${index + 1}/${count}: ${run.seconds}s, ${run.mebibytesPerSecond} MiB/s`);
}
const seconds = runs.map(run => run.seconds).sort((a, b) => a - b);
const rates = runs.map(run => run.mebibytesPerSecond).sort((a, b) => a - b);
const median = values => values.length % 2 ? values[Math.floor(values.length / 2)] : (values[values.length / 2 - 1] + values[values.length / 2]) / 2;
const report = {
  measuredAt: new Date().toISOString(), platform: process.platform, architecture: process.arch,
  scenario: 'Two Rust engines in one release-mode test process; TLS loopback; sparse zero-filled fixture; includes hashing, scripted approval and verification; excludes browser staging. Not a Wi-Fi or AirDrop result.',
  chunkBytes: 8 * 1024 * 1024, runs, medianSeconds: median(seconds),
  p95Seconds: seconds[Math.ceil(seconds.length * 0.95) - 1], medianMebibytesPerSecond: median(rates),
};
writeFileSync('BENCHMARK_RESULTS.json', `${JSON.stringify(report, null, 2)}\n`);
console.log(`Median: ${report.medianMebibytesPerSecond} MiB/s. p95 elapsed: ${report.p95Seconds}s. Results: BENCHMARK_RESULTS.json`);
