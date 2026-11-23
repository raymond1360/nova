DROP TRIGGER api_plan_immutable;
DROP INDEX api_single_active;
ALTER TABLE api_task_runs RENAME TO api_task_runs_v13;
CREATE TABLE api_task_runs (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), project_id TEXT NOT NULL REFERENCES nova_projects(id),
 plan_json TEXT NOT NULL, digest TEXT NOT NULL CHECK(length(digest)=64), phase TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>=0), summary_json TEXT NOT NULL
);
INSERT INTO api_task_runs SELECT * FROM api_task_runs_v13;
DROP TABLE api_task_runs_v13;
CREATE TABLE api_task_scopes (task_id TEXT PRIMARY KEY REFERENCES tasks(id), first_run TEXT NOT NULL UNIQUE REFERENCES api_task_runs(id), repair_run TEXT UNIQUE REFERENCES api_task_runs(id));
INSERT INTO api_task_scopes SELECT task_id,id,NULL FROM api_task_runs;
CREATE UNIQUE INDEX api_single_active ON api_task_runs((1)) WHERE phase NOT IN ('AWAITING_USER','NEEDS_REMEDIATION','FAILED','CANCELLED','ACCEPTED','USER_REJECTED');
CREATE TRIGGER api_plan_immutable BEFORE UPDATE OF task_id,project_id,plan_json,digest ON api_task_runs BEGIN SELECT RAISE(ABORT,'immutable API approval'); END;
CREATE TRIGGER api_run_no_delete BEFORE DELETE ON api_task_runs BEGIN SELECT RAISE(ABORT,'retain execution evidence'); END;
CREATE TRIGGER api_scope_no_delete BEFORE DELETE ON api_task_scopes BEGIN SELECT RAISE(ABORT,'retain repair limit'); END;
CREATE TRIGGER api_scope_immutable BEFORE UPDATE ON api_task_scopes WHEN NEW.task_id!=OLD.task_id OR NEW.first_run!=OLD.first_run OR OLD.repair_run IS NOT NULL BEGIN SELECT RAISE(ABORT,'immutable task scope'); END;
CREATE TABLE api_credential_cleanup (id TEXT PRIMARY KEY, slot TEXT NOT NULL, reason TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
