function sameComparisonClass(left, right) {
  return left.track === right.track
    && left.mode === right.mode
    && left.durability_contract === right.durability_contract
    && left.workload === right.workload
    && left.operation_count === right.operation_count
    && left.touched_path_count === right.touched_path_count
    && left.file_bytes === right.file_bytes
    && left.workers_effective === right.workers_effective;
}

const PROFILE_MODES = {
  publication: {
    weavatrix: ['dry-run', 'durable-apply'],
    atomwrite: ['dry-run'],
    'git-apply': ['dry-run', 'non-durable-apply'],
  },
  'atomwrite-durable-audit': {
    weavatrix: ['durable-apply'],
    atomwrite: ['durable-apply'],
    'git-apply': ['non-durable-apply'],
  },
};

export function comparisonModes(profile, adapter) {
  const modes = PROFILE_MODES[profile]?.[adapter];
  if (modes === undefined) {
    throw new Error(
      '--comparison-profile must be publication or atomwrite-durable-audit',
    );
  }
  return [...modes];
}

export function applyComparisonPublicationPolicy(profile, configurations) {
  if (profile !== 'atomwrite-durable-audit') return configurations;
  return configurations.map((configuration) => {
    if (configuration.adapter !== 'atomwrite'
      || configuration.mode !== 'durable-apply') {
      return configuration;
    }
    return {
      ...configuration,
      publishable: false,
      equivalent_comparison_eligible: false,
      publication_exclusion_reason:
        'ATOMWRITE_0_1_36_TRANSACTION_MODE_HAS_NO_BATCH_ROLLBACK_BACKUP_CLEANUP_CONTRACT',
      latency_statistics_class: 'DIAGNOSTIC_ONLY_NON_PUBLISHABLE',
    };
  });
}

export function atomwriteDurableAuditSummary(rows) {
  const samples = rows.filter((row) => row.adapter === 'atomwrite'
    && row.mode === 'durable-apply');
  const backupArtifactSamples = samples.filter((row) => row.unexpected_artifacts
    .some((entry) => /\.bak(?:[.-]|$)/u.test(entry.path)));
  const artifactGateFailures = samples.filter((row) => !row.gates.artifact_cleanup);
  return {
    mode: 'atomwrite:durable-apply',
    nonpublishable_by_policy: true,
    external_cleanup_performed: false,
    sample_count: samples.length,
    recorded_sample_count: samples.filter((row) => !row.warmup).length,
    tree_state_pass_samples: samples.filter((row) => row.gates.tree_state).length,
    artifact_cleanup_failure_samples: artifactGateFailures.length,
    rollback_backup_artifact_samples: backupArtifactSamples.length,
    contract_defect_reproduced: backupArtifactSamples.length > 0
      && backupArtifactSamples.every((row) => row.gates.tree_state),
    publication_exclusion_reason:
      'ATOMWRITE_0_1_36_TRANSACTION_MODE_HAS_NO_BATCH_ROLLBACK_BACKUP_CLEANUP_CONTRACT',
  };
}

export function conservativeTwoTimesGates(configurations) {
  const rows = [];
  const weavatrixRows = configurations.filter((row) => row.adapter === 'weavatrix');
  for (const weavatrix of weavatrixRows) {
    for (const competitor of configurations) {
      if (competitor.adapter === 'weavatrix'
        || !weavatrix.publishable
        || !competitor.publishable
        || !weavatrix.equivalent_comparison_eligible
        || !competitor.equivalent_comparison_eligible
        || !sameComparisonClass(weavatrix, competitor)
        || weavatrix.p75_ms === null
        || competitor.p25_ms === null) {
        continue;
      }
      const ratio = competitor.p25_ms / weavatrix.p75_ms;
      rows.push({
        competitor: competitor.adapter,
        mode: weavatrix.mode,
        durability_contract: weavatrix.durability_contract,
        workload: weavatrix.workload,
        operation_count: weavatrix.operation_count,
        touched_path_count: weavatrix.touched_path_count,
        file_bytes: weavatrix.file_bytes,
        workers_effective: weavatrix.workers_effective,
        competitor_p25_ms: competitor.p25_ms,
        weavatrix_p75_ms: weavatrix.p75_ms,
        ratio,
        passes_two_times: ratio >= 2,
      });
    }
  }
  return {
    formula: 'competitor_p25_ms / weavatrix_p75_ms >= 2.0',
    equal_contract_publishable_rows_only: true,
    eligible_rows: rows,
    all_eligible_rows_pass: rows.length > 0 && rows.every((row) => row.passes_two_times),
  };
}

function predeclaredWeakerMode(weavatrixMode, competitor) {
  if (weavatrixMode === 'dry-run') return 'dry-run';
  if (weavatrixMode === 'durable-apply' && competitor === 'atomwrite') {
    return 'durable-apply';
  }
  if (weavatrixMode === 'durable-apply' && competitor === 'git-apply') {
    return 'non-durable-apply';
  }
  return null;
}

function compatibleWorkers(weavatrix, competitor) {
  return competitor.workers_effective === null
    || competitor.workers_effective === weavatrix.workers_effective;
}

export function strongerContractPerformanceFloor(configurations) {
  const rows = [];
  const weavatrixRows = configurations.filter((row) => row.adapter === 'weavatrix');
  for (const weavatrix of weavatrixRows) {
    for (const competitor of configurations) {
      const weakerMode = predeclaredWeakerMode(weavatrix.mode, competitor.adapter);
      if (competitor.adapter === 'weavatrix'
        || weakerMode === null
        || competitor.mode !== weakerMode
        || !weavatrix.publishable
        || !competitor.publishable
        || weavatrix.track !== competitor.track
        || weavatrix.workload !== competitor.workload
        || weavatrix.operation_count !== competitor.operation_count
        || weavatrix.touched_path_count !== competitor.touched_path_count
        || weavatrix.file_bytes !== competitor.file_bytes
        || !compatibleWorkers(weavatrix, competitor)
        || weavatrix.p75_ms === null
        || competitor.p25_ms === null) {
        continue;
      }
      const ratio = competitor.p25_ms / weavatrix.p75_ms;
      rows.push({
        competitor: competitor.adapter,
        weavatrix_mode: weavatrix.mode,
        competitor_mode: competitor.mode,
        weavatrix_contract: weavatrix.durability_contract,
        competitor_contract: competitor.durability_contract,
        workload: weavatrix.workload,
        operation_count: weavatrix.operation_count,
        touched_path_count: weavatrix.touched_path_count,
        file_bytes: weavatrix.file_bytes,
        weavatrix_workers_effective: weavatrix.workers_effective,
        competitor_workers_effective: competitor.workers_effective,
        competitor_p25_ms: competitor.p25_ms,
        weavatrix_p75_ms: weavatrix.p75_ms,
        ratio,
        passes_two_times_floor: ratio >= 2,
        equivalent_contracts: false,
      });
    }
  }
  return {
    name: 'stronger_contract_performance_floor',
    formula: 'weaker_competitor_p25_ms / stronger_weavatrix_p75_ms >= 2.0',
    predeclared_modes: {
      'weavatrix:dry-run': ['atomwrite:dry-run', 'git-apply:dry-run'],
      'weavatrix:durable-apply': [
        'atomwrite:durable-apply', 'git-apply:non-durable-apply',
      ],
    },
    equivalent_contracts: false,
    universal_ranking: false,
    eligible_rows: rows,
    all_eligible_rows_pass: rows.length > 0
      && rows.every((row) => row.passes_two_times_floor),
  };
}
