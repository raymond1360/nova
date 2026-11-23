CREATE TABLE api_pm_packets (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, packet_json TEXT NOT NULL, digest TEXT NOT NULL,
 phase TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, task_id TEXT UNIQUE, run_id TEXT,
 approved_digest TEXT, pm_count INTEGER NOT NULL DEFAULT 0 CHECK(pm_count BETWEEN 0 AND 5),
 draft_json TEXT, error_code TEXT
);
CREATE UNIQUE INDEX api_pm_project_active ON api_pm_packets((1)) WHERE phase IN ('PREPARED','PLANNING_STARTED','DRAFT','CONFIRMING','READY','APPROVED','RUNNING','UNKNOWN');
CREATE TABLE api_pm_decisions (
 packet_id TEXT NOT NULL REFERENCES api_pm_packets(id), ordinal INTEGER NOT NULL,
 run_id TEXT, stage TEXT NOT NULL, input_json TEXT NOT NULL, input_hash TEXT NOT NULL,
 phase TEXT NOT NULL, output_json TEXT, disposition TEXT, rejection_reason TEXT, PRIMARY KEY(packet_id,ordinal)
);
CREATE TRIGGER api_pm_packet_identity BEFORE UPDATE OF packet_json,digest,project_id ON api_pm_packets BEGIN SELECT RAISE(ABORT,'immutable PM packet'); END;
CREATE TRIGGER api_pm_packet_no_delete BEFORE DELETE ON api_pm_packets BEGIN SELECT RAISE(ABORT,'retained PM packet'); END;
CREATE TRIGGER api_pm_decision_no_delete BEFORE DELETE ON api_pm_decisions BEGIN SELECT RAISE(ABORT,'retained PM decision'); END;

CREATE UNIQUE INDEX api_pm_stage_once ON api_pm_decisions(packet_id,run_id,stage) WHERE run_id IS NOT NULL;
