//! Fixed canary subset, not a general Codex configuration loader.
//! Real cloud/thread provenance is not yet observable without auth/session access.
use super::*;
const REQUIRED: &str = "SELF2_CANARY_TRUSTED_CONFIGURATION_REQUIRED";
const SOURCES: &[&str] = &["package","system","managed","cloud","user","profile","cwd","ancestor","repo","runtime","thread"];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="SCREAMING_SNAKE_CASE")]
enum State { Absent, Ignored, Frozen, Unknown, Unsupported }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    schema: String,
    binding: String,
    sources: BTreeMap<String, State>,
    // Only finite, non-secret effective capability facts. Never auth or raw config.
    capabilities: BTreeMap<String, String>,
    observed_utc: i64,
    expires_utc: i64,
}
// No Deserialize/public constructor: a caller-provided reference cannot create authority.
pub(super) struct TrustedConfiguration { digest: String, pin: Guard }
impl TrustedConfiguration {
    pub(super) fn digest(&self)->&str {&self.digest}
    pub(super) fn verify(&self)->Result<()> { self.pin.verify() }
}
fn source_read()->Result<(Observed, Guard)> {
    #[cfg(test)]
    if let Some(path)=TEST_SOURCE.with(|v|v.borrow().clone()) {
        let pin=Guard::open(&path,false)?;
        let raw=super::super::store::read_file(&path,65536)?.0;
        let observed=serde_json::from_slice(&raw).map_err(|_|"SELF2_CANARY_CONFIG_EVIDENCE_REJECTED")?;
        pin.verify()?;
        return Ok((observed,pin))
    }
    // exec/src/lib.rs constructs cloud_config_bundle_loader_for_storage from bootstrap auth,
    // and config loader loads thread overrides. No absence proof is inferred from local paths.
    // Fail before reading machine config/auth/session or offering any native launch.
    Err(REQUIRED)
}
fn capabilities()->BTreeMap<String,String> {
    [("mcp","EMPTY"),("extra_tools","NONE"),("plugins","OFF"),("hooks","OFF"),
     ("agents","OFF"),("search","OFF"),("provider","OPENAI_BUILTIN"),
     ("endpoints","OFFICIAL_DEFAULTS"),("telemetry","NO_CUSTOM_SINK"),
     ("auth_method","CHATGPT_EXISTING"),("windows_backend","UNELEVATED"),
     ("develop","WORKSPACE_WRITE"),("review","READ_ONLY"),("resume_overrides","NONE")]
        .into_iter().map(|(k,v)|(k.into(),v.into())).collect()
}
fn validate(observed:&Observed)->Result<()> {
    let now=chrono::Utc::now().timestamp();
    if observed.schema!="nova.self2.fixed-config-observation/1" {return Err("SELF2_CANARY_CONFIG_EVIDENCE_REJECTED")}
    if observed.observed_utc>now
        || observed.expires_utc<=now || observed.expires_utc.checked_sub(observed.observed_utc).is_none_or(|age|age>86400) {
        return Err("SELF2_CANARY_CONFIG_EVIDENCE_EXPIRED")
    }
    if observed.sources.len()!=SOURCES.len(){return Err("SELF2_CANARY_CONFIG_SOURCE_UNSUPPORTED")}
    for name in SOURCES {
        let expected=match *name {"user"=>State::Ignored,"runtime"=>State::Frozen,_=>State::Absent};
        if observed.sources.get(*name)!=Some(&expected){return Err("SELF2_CANARY_CONFIG_SOURCE_UNSUPPORTED")}
    }
    if observed.capabilities!=capabilities(){return Err("SELF2_CANARY_CONFIG_CAPABILITY_REJECTED")}
    Ok(())
}
// Stable approval context; the actual resume UUID is separately checked against the tool receipt.
fn context(s:&Store,request:&CanaryRequest,scope:&super::super::admission_probe::PermissionScope)->Result<String> {
    let j=journal(s)?;let c=request.conditions.as_ref().ok_or("SELF2_CANARY_CONDITIONS_PENDING")?;
    canary_conditions(&request.conditions,&j.plan)?;
    if request.expires_utc<=chrono::Utc::now().timestamp(){return Err("SELF2_CANARY_CONFIG_EVIDENCE_EXPIRED")}
    let actual=std::env::current_exe().map_err(|_|"SELF_PATH")?;
    if actual!=request.consumer_path || tool_digest(&actual)?!=request.consumer_sha256
        || j.plan_sha256!=request.plan_sha256 || j.plan.project_root!=scope.project_root
        || j.plan.cli!=scope.cli || j.plan.cli_sha256!=scope.cli_sha256 {
        return Err("SELF2_CANARY_CONFIG_BINDING_MISMATCH")
    }
    let permission=super::super::admission_probe::bind_permission(scope)?;
    if permission.fingerprint()!=request.permission_fingerprint {return Err("SELF2_CANARY_CONFIG_BINDING_MISMATCH")}
    let root=Guard::open(&j.plan.project_root,true)?;
    let schema=h::store_path(&j.plan.project_root)?.join("backups/self2").join("output.schema.json");
    let parent=cli_adapter::ENV_ALLOW.iter().filter_map(|k|std::env::var(k).ok().map(|v|(k.to_string(),v))).collect();
    let env=canary_environment(&parent,schema.parent().ok_or("PATH_REJECTED")?,c)?;
    let mut request_value=serde_json::to_value(request).map_err(|_|"ENCODING")?;
    request_value.as_object_mut().ok_or("ENCODING")?.remove("configuration_sha256");
    let d=canary_args(&j.plan,c,Role::Develop,None,&schema)?;
    let r=canary_args(&j.plan,c,Role::Review,None,&schema)?;
    let mut resume=canary_args(&j.plan,c,Role::Review,Some("00000000-0000-4000-8000-000000000001"),&schema)?;
    for a in &mut resume {if a=="00000000-0000-4000-8000-000000000001" {*a="BOUND_NEW_REVIEW_UUID".into()}}
    root.verify()?;
    Ok(hash(&bytes(&json!({"request":request_value,"root_identity":root.identity,
        "home":canary_auth_home(true)?,"cli":j.plan.cli,"cli_sha256":j.plan.cli_sha256,
        "stage":j.plan.input.stage,"package":j.plan.input.package,"env":env,
        "d1":d,"r1":r,"r2":resume}))?))
}
pub(super) fn available()->Result<()> {let (o,_pin)=source_read()?;validate(&o)}
pub(super) fn read(s:&Store,request:&CanaryRequest,scope:&super::super::admission_probe::PermissionScope,issue:bool)->Result<TrustedConfiguration> {
    let (o,pin)=source_read()?;validate(&o)?;
    if o.binding!=context(s,request,scope)? || o.expires_utc<request.expires_utc {
        return Err("SELF2_CANARY_CONFIG_BINDING_MISMATCH")
    }
    let digest=hash(&bytes(&o)?);
    if !issue && request.configuration_sha256.as_deref()!=Some(digest.as_str()) {
        return Err("SELF2_CANARY_CONFIG_DRIFT")
    }
    pin.verify()?;Ok(TrustedConfiguration {digest,pin})
}
pub(super) fn spec(s:&Store,request:&CanaryRequest,value:&Spec)->Result<()> {
    let j=journal(s)?;
    let completed=j.records.iter().filter(|r|r.outcome=="COMPLETE").count();
    let stage=h::load(s)?.1.stage;
    let expected_role=if completed==0 {RelayRole::Develop}else{RelayRole::Review};
    let expected_session=if completed==2 {stage.reviewer.as_ref().ok_or("SELF2_UUID_MISSING")?.uuid.as_str()}else{""};
    if completed>2 || value.role!=expected_role || value.session!=expected_session {
        return Err("SELF2_CANARY_CONFIG_SPEC_MISMATCH")
    }
    let p=j.plan;let c=request.conditions.as_ref().ok_or("SELF2_CANARY_CONDITIONS_PENDING")?;
    let role=match value.role {RelayRole::Develop=>Role::Develop,RelayRole::Review=>Role::Review};
    let schema=h::store_path(&p.project_root)?.join("backups/self2").join("output.schema.json");
    let parent=cli_adapter::ENV_ALLOW.iter().filter_map(|k|std::env::var(k).ok().map(|v|(k.to_string(),v))).collect();
    let expected=canary_args(&p,c,role,if value.session.is_empty(){None}else{Some(&value.session)},&schema)?;
    if value.profile!=role.profile() || value.exe!=p.cli || value.exe_sha256!=p.cli_sha256 || value.cwd!=p.project_root
        || value.argv!=expected || value.env!=canary_environment(&parent,schema.parent().ok_or("PATH_REJECTED")?,c)? {
        return Err("SELF2_CANARY_CONFIG_SPEC_MISMATCH")
    }
    Ok(())
}
#[cfg(test)]
thread_local! {static TEST_SOURCE:std::cell::RefCell<Option<PathBuf>>=const {std::cell::RefCell::new(None)}; static TEST_SCOPE:std::cell::RefCell<Option<super::super::admission_probe::PermissionScope>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
pub(super) struct TestSource { previous:Option<PathBuf>, previous_scope:Option<super::super::admission_probe::PermissionScope>, path:PathBuf, original:Observed }
#[cfg(test)]
impl Drop for TestSource {fn drop(&mut self){TEST_SOURCE.with(|v|{v.replace(self.previous.take());});TEST_SCOPE.with(|v|{v.replace(self.previous_scope.take());});}}
#[cfg(test)]
pub(super) fn test_source(s:&Store,request:&CanaryRequest,scope:&super::super::admission_probe::PermissionScope)->TestSource {
    let dir=std::env::temp_dir().join(format!("nova-config-source-{}",uuid::Uuid::new_v4()));fs::create_dir(&dir).unwrap();
    let path=dir.join("synthetic-observation.json");let original=Observed {schema:"nova.self2.fixed-config-observation/1".into(),binding:context(s,request,scope).unwrap(),sources:SOURCES.iter().map(|n|((*n).into(),match *n {"user"=>State::Ignored,"runtime"=>State::Frozen,_=>State::Absent})).collect(),capabilities:capabilities(),observed_utc:chrono::Utc::now().timestamp(),expires_utc:request.expires_utc};
    fs::write(&path,bytes(&original).unwrap()).unwrap();let previous=TEST_SOURCE.with(|v|v.replace(Some(path.clone())));
    let previous_scope=TEST_SCOPE.with(|v|v.replace(Some(scope.clone())));
    TestSource {previous,previous_scope,path,original}
}
#[cfg(test)]
impl TestSource {
    pub(super) fn fault(&self,name:&str) {
        let mut o=self.original.clone();
        match name {"valid"=>{},"binding"=>o.binding="0".repeat(64),"expired"=>o.expires_utc=0,
            "mcp"=>{o.capabilities.insert("mcp".into(),"EXTRA".into());},
            "provider"=>{o.capabilities.insert("provider".into(),"OTHER".into());},
            "endpoint"=>{o.capabilities.insert("endpoints".into(),"OTHER".into());},
            "permission"=>{o.capabilities.insert("review".into(),"WRITE".into());},
            x if x.starts_with("cap-")=>{o.capabilities.insert(x[4..].into(),"EXTRA".into());},
            x=>{o.sources.insert(x.into(),State::Unknown);} }
        fs::write(&self.path,bytes(&o).unwrap()).unwrap();
    }
}

#[cfg(test)]
pub(super) fn test_scope(s:&Store)->Result<super::super::admission_probe::PermissionScope> {
    let scope=TEST_SCOPE.with(|v|v.borrow().clone()).ok_or(REQUIRED)?;
    if scope.project_root!=journal(s)?.plan.project_root{return Err("SELF2_CANARY_CONFIG_BINDING_MISMATCH")}
    Ok(scope)
}

#[cfg(test)]
pub(super) fn test_rotate_source() {
    let path=TEST_SOURCE.with(|v|v.borrow().clone()).unwrap();
    let mut o:Observed=serde_json::from_slice(&super::super::store::read_file(&path,65536).unwrap().0).unwrap();
    o.capabilities.insert("mcp".into(),"DRIFT".into());
    let next=path.parent().unwrap().join(format!("drift-{}.json",uuid::Uuid::new_v4()));
    fs::write(&next,bytes(&o).unwrap()).unwrap();TEST_SOURCE.with(|v|{v.replace(Some(next));});
}
#[cfg(test)]
pub(super) fn test_spec_rejections(s:&Store,request:&CanaryRequest) {
    let p=journal(s).unwrap().plan;let c=request.conditions.as_ref().unwrap();
    let schema=h::store_path(&p.project_root).unwrap().join("backups/self2").join("output.schema.json");
    let parent=cli_adapter::ENV_ALLOW.iter().filter_map(|k|std::env::var(k).ok().map(|v|(k.to_string(),v))).collect();
    let value=Spec {role:RelayRole::Develop,profile:Role::Develop.profile(),exe:p.cli.clone(),exe_sha256:p.cli_sha256.clone(),argv:canary_args(&p,c,Role::Develop,None,&schema).unwrap(),env:canary_environment(&parent,schema.parent().unwrap(),c).unwrap(),cwd:p.project_root,session:String::new()};
    spec(s,request,&value).unwrap();
    for fault in ["argv","env","cwd","exe","profile","session"] {
        let mut bad=value.clone();match fault {"argv"=>bad.argv.push("EXTRA".into()),"env"=>{bad.env.insert("EXTRA".into(),"X".into());},"cwd"=>bad.cwd="C:/wrong".into(),"exe"=>bad.exe="C:/wrong.exe".into(),"session"=>bad.session="00000000-0000-4000-8000-000000000002".into(),_=>bad.profile="WRONG".into()};
        assert_eq!(spec(s,request,&bad),Err("SELF2_CANARY_CONFIG_SPEC_MISMATCH"));
    }
}
