CREATE TABLE api_model_configs (
 id TEXT PRIMARY KEY, config_json TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
 has_key INTEGER NOT NULL CHECK(has_key IN(0,1)), credential_slot TEXT NOT NULL
);
CREATE TABLE api_task_runs (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
 project_id TEXT NOT NULL REFERENCES nova_projects(id), plan_json TEXT NOT NULL,
 digest TEXT NOT NULL CHECK(length(digest)=64), phase TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision>=0), summary_json TEXT NOT NULL
);
CREATE UNIQUE INDEX api_single_active ON api_task_runs((1))
 WHERE phase NOT IN ('AWAITING_USER','NEEDS_REMEDIATION','FAILED','CANCELLED');
CREATE TRIGGER api_plan_immutable BEFORE UPDATE OF task_id,project_id,plan_json,digest ON api_task_runs
 BEGIN SELECT RAISE(ABORT,'immutable API approval'); END;
