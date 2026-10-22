CREATE TABLE requirement_conversations (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL UNIQUE REFERENCES nova_projects(id),
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(id,project_id)
);
CREATE TABLE requirement_drafts (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, conversation_id TEXT NOT NULL,
 origin_task_id TEXT, kind TEXT NOT NULL CHECK(kind IN ('initial','next')),
 status TEXT NOT NULL CHECK(status IN ('editable','bound','archived')),
 current_revision INTEGER NOT NULL CHECK(current_revision>0), created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(id,project_id), UNIQUE(id,project_id,conversation_id),
 FOREIGN KEY(conversation_id,project_id) REFERENCES requirement_conversations(id,project_id),
 FOREIGN KEY(origin_task_id,project_id) REFERENCES tasks(id,project_id),
 FOREIGN KEY(id,current_revision,project_id) REFERENCES requirement_revisions(draft_id,revision,project_id) DEFERRABLE INITIALLY DEFERRED,
 CHECK((kind='initial' AND origin_task_id IS NULL) OR (kind='next' AND origin_task_id IS NOT NULL))
);
CREATE UNIQUE INDEX requirement_initial ON requirement_drafts(project_id) WHERE kind='initial' AND status='editable';
CREATE UNIQUE INDEX requirement_next ON requirement_drafts(project_id,origin_task_id) WHERE kind='next' AND status='editable';
CREATE INDEX requirement_draft_list ON requirement_drafts(project_id,status,updated_at);
CREATE TABLE requirement_revisions (
 draft_id TEXT NOT NULL, project_id TEXT NOT NULL, revision INTEGER NOT NULL, base_revision INTEGER NOT NULL,
 title_raw TEXT NOT NULL, selected_ai TEXT CHECK(selected_ai IS NULL OR selected_ai IN ('codex_cli','claude_cli')),
 goal_raw TEXT NOT NULL, in_scope_raw TEXT NOT NULL, not_in_scope_raw TEXT NOT NULL, done_criteria_raw TEXT NOT NULL,
 source TEXT NOT NULL CHECK(source IN ('user','ai_applied','mixed')), source_planning_id TEXT,
 blocking_questions_json TEXT NOT NULL DEFAULT '[]', content_hash TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(draft_id,revision), UNIQUE(draft_id,revision,project_id),
 FOREIGN KEY(draft_id,project_id) REFERENCES requirement_drafts(id,project_id),
 FOREIGN KEY(source_planning_id,project_id) REFERENCES planning_requests(id,project_id)
);
CREATE TABLE requirement_messages (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, conversation_id TEXT NOT NULL, seq INTEGER NOT NULL,
 draft_id TEXT, revision INTEGER, kind TEXT NOT NULL, body_raw TEXT, payload_json TEXT, planning_request_id TEXT, created_at TEXT NOT NULL,
 UNIQUE(conversation_id,seq),
 FOREIGN KEY(conversation_id,project_id) REFERENCES requirement_conversations(id,project_id),
 FOREIGN KEY(draft_id,project_id,conversation_id) REFERENCES requirement_drafts(id,project_id,conversation_id),
 FOREIGN KEY(draft_id,revision,project_id) REFERENCES requirement_revisions(draft_id,revision,project_id),
 FOREIGN KEY(planning_request_id,project_id) REFERENCES planning_requests(id,project_id)
);
CREATE TABLE planning_authorizations (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, conversation_id TEXT NOT NULL, selected_ai TEXT NOT NULL,
 binary_identity_hash TEXT NOT NULL, policy_version TEXT NOT NULL, context_scope_json TEXT NOT NULL, scope_hash TEXT NOT NULL,
 granted_at TEXT NOT NULL, expires_at TEXT NOT NULL, revoked_at TEXT, UNIQUE(id,project_id),
 FOREIGN KEY(conversation_id,project_id) REFERENCES requirement_conversations(id,project_id)
);
CREATE TABLE planning_requests (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, draft_id TEXT NOT NULL, source_revision INTEGER NOT NULL,
 authorization_id TEXT NOT NULL, client_request_id TEXT NOT NULL, request_hash TEXT NOT NULL, selected_ai TEXT NOT NULL,
 binary_identity_hash TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('reserved','dispatching','running','uncertain','succeeded','failed','cancelled')),
 dispatch_fence TEXT, context_manifest_json TEXT NOT NULL, input_hash TEXT NOT NULL, output_json TEXT, output_hash TEXT,
 adoption_status TEXT NOT NULL, applied_revision INTEGER, usage_json TEXT, error_code TEXT, redacted_diagnostic TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(id,project_id), UNIQUE(project_id,client_request_id),
 FOREIGN KEY(draft_id,source_revision,project_id) REFERENCES requirement_revisions(draft_id,revision,project_id),
 FOREIGN KEY(authorization_id,project_id) REFERENCES planning_authorizations(id,project_id)
);
CREATE UNIQUE INDEX planning_unsettled ON planning_requests(project_id) WHERE status IN ('reserved','dispatching','running','uncertain');
CREATE INDEX planning_revision ON planning_requests(draft_id,source_revision,created_at);
CREATE TABLE requirement_task_bindings (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL, draft_id TEXT NOT NULL UNIQUE, revision INTEGER NOT NULL,
 content_hash TEXT NOT NULL, selected_ai TEXT NOT NULL, task_id TEXT NOT NULL UNIQUE, confirmed_at TEXT NOT NULL,
 FOREIGN KEY(draft_id,revision,project_id) REFERENCES requirement_revisions(draft_id,revision,project_id),
 FOREIGN KEY(task_id,project_id) REFERENCES tasks(id,project_id)
);
CREATE TABLE requirement_mutation_receipts (
 project_id TEXT NOT NULL REFERENCES nova_projects(id), operation TEXT NOT NULL, client_request_id TEXT NOT NULL,
 request_hash TEXT NOT NULL, resource_id TEXT NOT NULL, response_refs_json TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(project_id,operation,client_request_id)
);
CREATE TRIGGER requirement_revision_immutable_update BEFORE UPDATE ON requirement_revisions BEGIN SELECT RAISE(ABORT,'immutable revision'); END;
CREATE TRIGGER requirement_revision_immutable_delete BEFORE DELETE ON requirement_revisions BEGIN SELECT RAISE(ABORT,'immutable revision'); END;
CREATE TRIGGER requirement_message_immutable_update BEFORE UPDATE ON requirement_messages BEGIN SELECT RAISE(ABORT,'immutable message'); END;
CREATE TRIGGER requirement_message_immutable_delete BEFORE DELETE ON requirement_messages BEGIN SELECT RAISE(ABORT,'immutable message'); END;
CREATE TRIGGER requirement_binding_immutable_update BEFORE UPDATE ON requirement_task_bindings BEGIN SELECT RAISE(ABORT,'immutable binding'); END;
CREATE TRIGGER requirement_binding_immutable_delete BEFORE DELETE ON requirement_task_bindings BEGIN SELECT RAISE(ABORT,'immutable binding'); END;
CREATE TRIGGER requirement_receipt_immutable_update BEFORE UPDATE ON requirement_mutation_receipts BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
CREATE TRIGGER requirement_receipt_immutable_delete BEFORE DELETE ON requirement_mutation_receipts BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
