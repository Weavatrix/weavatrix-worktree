function cell(value) {
  if (value === null || value === undefined) return '—';
  return String(value).replaceAll('|', '\\|').replaceAll('\n', ' ');
}

function milliseconds(value) {
  return value === null || value === undefined ? '—' : Number(value).toFixed(3);
}

function result(value) {
  return value ? 'pass' : 'fail';
}

function yesNo(value) {
  return value ? 'yes' : 'no';
}

export function markdownComparisonReport(summary, config) {
  const lines = [
    '# Interleaved benchmark report',
    '',
    `- Run: \`${cell(config.run_id)}\``,
    `- Profile: \`${cell(config.comparison_profile)}\``,
    `- Schedule: ${summary.interleaved_tool_order ? 'interleaved' : 'not interleaved'}`,
    `- Samples: ${config.warmups} warmups + ${config.repetitions} recorded per configuration`,
    `- File size: ${config.file_bytes} bytes`,
    `- Universal ranking: ${summary.universal_ranking}`,
    '',
    '## Configuration results',
    '',
    '| Tool/version | Mode | Workload | Operations / paths | Workers requested / effective | Recorded / valid | p50 ms | p95 ms | Gates | Publishable |',
    '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |',
  ];
  for (const row of summary.configurations) {
    lines.push([
      `| ${cell(`${row.adapter} ${row.adapter_version}`)}`,
      cell(row.mode),
      cell(row.workload),
      `${cell(row.operation_count)} / ${cell(row.touched_path_count)}`,
      `${cell(row.workers_requested)} / ${cell(row.workers_effective)}`,
      `${cell(row.recorded_samples)} / ${cell(row.valid_samples)}`,
      milliseconds(row.p50_ms),
      milliseconds(row.p95_ms),
      result(row.all_correctness_gates_pass),
      yesNo(row.publishable),
    ].join(' | ') + ' |');
  }

  const strict = summary.two_times_gate;
  lines.push(
    '',
    '## Strict equal-contract 2× gate',
    '',
    `- Eligible rows: ${strict.eligible_rows.length}`,
    `- All eligible rows pass: ${strict.all_eligible_rows_pass}`,
  );
  if (strict.eligible_rows.length === 0) {
    lines.push('- Conclusion: no contract-equivalent competitor row exists; no strict 2× claim is eligible.');
  }

  const floor = summary.stronger_contract_performance_floor;
  lines.push(
    '',
    '## Stronger-contract performance floor',
    '',
    '- Contracts equivalent: false',
    '- Universal ranking: false',
    `- Eligible rows: ${floor.eligible_rows.length}`,
    `- All eligible rows pass: ${floor.all_eligible_rows_pass}`,
    '',
    '| Competitor | Modes | Workload | Operations / paths | Workers | Competitor p25 ms | Weavatrix p75 ms | Ratio | 2× floor |',
    '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |',
  );
  for (const row of floor.eligible_rows) {
    lines.push([
      `| ${cell(row.competitor)}`,
      cell(`${row.weavatrix_mode} / ${row.competitor_mode}`),
      cell(row.workload),
      `${cell(row.operation_count)} / ${cell(row.touched_path_count)}`,
      `${cell(row.weavatrix_workers_effective)} / ${cell(row.competitor_workers_effective)}`,
      milliseconds(row.competitor_p25_ms),
      milliseconds(row.weavatrix_p75_ms),
      Number(row.ratio).toFixed(3),
      result(row.passes_two_times_floor),
    ].join(' | ') + ' |');
  }

  if (summary.atomwrite_durable_audit !== undefined) {
    const audit = summary.atomwrite_durable_audit;
    lines.push(
      '',
      '## Atomwrite durable audit',
      '',
      `- Non-publishable by policy: ${audit.nonpublishable_by_policy}`,
      `- External cleanup performed: ${audit.external_cleanup_performed}`,
      `- Tree-state passes: ${audit.tree_state_pass_samples}/${audit.sample_count}`,
      `- Artifact-cleanup failures: ${audit.artifact_cleanup_failure_samples}/${audit.sample_count}`,
      `- Samples with rollback backups: ${audit.rollback_backup_artifact_samples}/${audit.sample_count}`,
      `- Contract defect reproduced: ${audit.contract_defect_reproduced}`,
    );
  }
  lines.push('', 'Raw evidence: `samples.jsonl`, `samples.csv`, `summary.json`, `config.json`, `machine.json`, `schedule.json`, and `rounds/`.', '');
  return lines.join('\n');
}
