//! Fixed content/Python-unit/Nova-Vitest recipes. Project recipes are not a sandbox.
use super::*;
pub fn freeze(
    t: &FixedTest,
    root: &Path,
) -> Result<(
    Vec<String>,
    std::collections::BTreeMap<String, String>,
    String,
    PathBuf,
)> {
    let mut env = std::collections::BTreeMap::new();
    #[cfg(windows)]
    {
        let system = std::env::var("SystemRoot").map_err(|_| "API_TEST_ENVIRONMENT_MISSING")?;
        if !Path::new(&system).is_absolute() || system.contains('\0') {
            return Err("API_TEST_ENVIRONMENT_MISSING");
        };
        env.insert("SystemRoot".into(), system.clone());
        env.insert("WINDIR".into(), system);
    }
    let mut cwd = root.to_path_buf();
    let is_node = t
        .program
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("node.exe"));
    if is_node != (t.recipe == "nova_vitest_api2") {
        return Err("API_TEST_PROGRAM_RECIPE_MISMATCH");
    }
    let (argv,notice)=match t.recipe.as_str(){
 ""|"content"=>(vec!["-I".into(),"-S".into(),"-c".into(),CHECKER.into(),root.join(&t.file).to_string_lossy().into(),t.contains.clone()],"Read-only content assertion. Trust selected Python installation including adjacent DLL/stdlib; executable hash does not cover dependencies."),
 "python_unittest"=>{let _guard=Guard::open(&root.join("tests"),true).map_err(|_|"API_UNITTEST_DIRECTORY_MISSING")?;(vec!["-I".into(),"-S".into(),"-B".into(),"-m".into(),"unittest".into(),"discover".into(),"-s".into(),"tests".into(),"-p".into(),"test_*.py".into()],"Executes project tests and their imports with current user permissions; may write files/spawn children/access network. Trust this project's code/dependencies and selected Python DLL/stdlib; program hash is not dependency proof. No installation or environment repair. NOT an OS sandbox.")},
 "nova_vitest_api2"=>{cwd=root.join("apps/desktop");let _dir=Guard::open(&cwd,true).map_err(|_|"API_NOVA_TEST_DEPENDENCY_MISSING")?;let _module=Guard::open(&cwd.join("node_modules/vitest/vitest.mjs"),false).map_err(|_|"API_NOVA_TEST_DEPENDENCY_MISSING")?;(vec!["node_modules/vitest/vitest.mjs".into(),"run".into(),"src/apiTaskPanel.interaction.test.ts".into(),"--maxWorkers=1".into(),"--minWorkers=1".into()],"Nova fixed Vitest API-panel suite executes trusted project/Vite configuration, node_modules and tests, may write caches/spawn/access network. No installation; NOT an OS sandbox. Trust Node runtime and project dependencies; program hash does not cover them.")},
 _=>return Err("API_TEST_RECIPE_UNSUPPORTED")};
    Ok((argv, env, notice.into(), cwd))
}
