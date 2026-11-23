<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import type { Task } from '../types'
const props=defineProps<{task:Task}>();const emit=defineEmits<{refresh:[]}>()
const catalog=ref<any[]>([]),configs=ref<any[]>([]),configId=ref(''),provider=ref('deepseek'),base=ref('https://api.deepseek.com/v1'),model=ref(''),key=ref('')
const paths=ref(''),program=ref(''),recipe=ref('nova_vitest_api2'),contains=ref(''),allowRepair=ref(false),riskAccepted=ref(false)
const plan=ref<any>(null),run=ref<any>(null),report=ref<any>(null),busy=ref(false),message=ref(''),error=ref(''),diagnostic=ref(''),feedback=ref(''),cleanup=ref(0)
const selectedConfig=computed(()=>configs.value.find(c=>c.config.id===configId.value))
const locked=computed(()=>busy.value||!!run.value&&['PREPARED','APPROVED','DEVELOP_STARTED','WRITE_INTENT','TEST_STARTED','REVIEW_STARTED','UNKNOWN','AWAITING_USER'].includes(run.value.phase))
const labels:Record<string,string>={PREPARED:'请核对执行范围',APPROVED:'已批准，尚未执行',DEVELOP_STARTED:'开发请求已预约；刷新不会重发',WRITE_INTENT:'写入已预约；结果待核对',TEST_STARTED:'测试进行中或待核对',REVIEW_STARTED:'独立自检进行中或待核对',UNKNOWN:'结果未知：已停止，需人工核对，不可重发',FAILED:'本轮失败，未完成',NEEDS_REMEDIATION:'需要整改',AWAITING_USER:'技术检查通过，等待您验收',ACCEPTED:'您已验收，任务已完成',USER_REJECTED:'您已提出修改，尚未完成',CANCELLED:'计划已取消，未执行；原记录保留，继续工作请创建新任务'}
function friendly(e:unknown){const c=String(e);if(c.includes('OFFICIAL_URL'))return '官方API地址不匹配，请使用服务商预设地址；自定义地址请选择兼容服务。';if(c.includes('CONFIG_INVALID')||c.includes('BASE_URL'))return '请填写有效的HTTPS API地址及明确模型名称。';if(c.includes('NOVA_TEST_DEPENDENCY_MISSING'))return 'Nova固定测试需要现有的apps/desktop/node_modules依赖，本工具不会自动安装。';if(c.includes('DRIFT'))return '文件、任务或配置已发生变化；已停止，请核对当前材料。';if(c.includes('REPAIR_LIMIT'))return '一次整改额度已使用，请交人工处理。';if(c.includes('ALREADY')||c.includes('RESERVED'))return '该任务已有记录，不能重新执行。请刷新查看。';if(c.includes('DIRECTORY_MISSING'))return '项目中缺少 tests 文件夹，请选择已具备测试环境的项目。';if(c.includes('ADAPTER_NOT_IMPLEMENTED'))return '该服务适配器尚未实现，目前仅DeepSeek可执行。';if(c.includes('APPROVAL'))return '批准无效、已过期或范围不符，请重新核对执行范围。';return '操作未完成，请查看详细原因并保留当前记录。'}
async function action(fn:()=>Promise<void>){error.value='';diagnostic.value='';message.value='';try{await fn()}catch(e){error.value=friendly(e);diagnostic.value=String(e)}}
async function load(){const previousPlan=plan.value?.id;const r=await invoke<any>('api_only_task_report',{taskId:props.task.id});report.value=r;run.value=r.runs.at(-1)??null;plan.value=r.plans.at(-1)??null;if(previousPlan!==plan.value?.id)riskAccepted.value=false;configs.value=await invoke<any[]>('api_only_configs');if(plan.value&&!configId.value){configId.value=plan.value.config.config.id;selectConfig();paths.value=plan.value.files.map((f:any)=>f.path).join('\n');program.value=plan.value.tests[0]?.specification.program??'';recipe.value=plan.value.tests[0]?.specification.recipe||'content';contains.value=plan.value.tests[0]?.specification.contains??''}cleanup.value=(await invoke<any>('api_only_cleanup_status')).pending}
onMounted(()=>action(async()=>{catalog.value=await invoke<any[]>('api_only_catalog');await load()}))
function preset(){const p=catalog.value.find(c=>c.provider===provider.value);base.value=p?.base_url??'';configId.value='';model.value='';key.value=''}
function selectConfig(){key.value='';const c=configs.value.find(c=>c.config.id===configId.value)?.config;if(c){provider.value=c.provider;base.value=c.base_url;model.value=c.model}}
async function save(){await action(async()=>{try{if(!model.value.trim()){error.value='请填写模型名称。';return}let u:URL;try{u=new URL(base.value)}catch{error.value='请输入有效的API地址。';return}if(u.protocol!=='https:'||u.username||u.password||u.search||u.hash){error.value='API地址须为HTTPS，且不能包含账号、查询参数或片段。';return}const c=await invoke<any>('api_only_save_config',{config:{id:configId.value||crypto.randomUUID(),provider:provider.value,base_url:base.value,model:model.value},key:key.value||null});configId.value=c.config.id;await load();message.value='配置已保存；没有发起网络请求。'}finally{key.value=''}})}
async function deleteKey(){await action(async()=>{try{await invoke('api_only_delete_key',{id:configId.value});await load();message.value='所选Key已删除。'}finally{key.value=''}})}
async function connect(){await action(async()=>{const r=await invoke<any>('api_only_test_connection',{id:configId.value,explicitNetwork:true});message.value=r.reachable?'接口可达；开发能力尚未验证。':'连接未确认。'})}
function selectedPaths(){return paths.value.split(/\r?\n/).map(s=>s.trim()).filter(Boolean)}
async function chooseProgram(){await action(async()=>{const p=await open({multiple:false,filters:[{name:'测试程序',extensions:['exe']}]});if(typeof p==='string')program.value=p})}
async function prepare(repair=false){await action(async()=>{const list=selectedPaths();if(!configId.value||!list.length||!program.value||(recipe.value==='content'&&!contains.value)){error.value='请选择已保存配置、文件、测试程序及所需断言内容。';return}const tests=[{program:program.value,file:list[0],contains:contains.value||'project unittest',recipe:recipe.value}];plan.value=await invoke<any>(repair?'api_only_repair':'api_only_prepare',repair?{parent:run.value.run_id,paths:list,tests,configId:configId.value}:{taskId:props.task.id,configId:configId.value,paths:list,tests,allowRepair:allowRepair.value});riskAccepted.value=false;await load()})}
async function approve(){await action(async()=>{if(!riskAccepted.value)return;run.value=await invoke<any>('api_only_approve',{id:run.value.run_id,digest:run.value.digest});message.value='范围已批准。请点击执行；不会因刷新自动开始。'})}
async function execute(){if(busy.value)return;busy.value=true;await action(async()=>{try{run.value=await invoke<any>('api_only_execute',{id:run.value.run_id})}finally{await load();emit('refresh')}});busy.value=false}
async function cancel(){await action(async()=>{await invoke('cancel_core_loop',{taskId:props.task.id});await load();message.value='已请求取消；已发送请求可能仍计费，未知结果不会自动重发。'})}
async function decide(accepted:boolean){await action(async()=>{if(!accepted&&!feedback.value.trim()){error.value='请写下需要修改的内容。';return}const useGrant=!accepted&&plan.value?.allow_repair&&report.value?.repair_count===0;await invoke('api_only_user_decision',{id:run.value.run_id,accepted,note:feedback.value});await load();emit('refresh');if(useGrant){await execute()}})}
</script>
<template>
<p v-if="report?.pm_packet_id">本任务由上方产品经理协调区处理；不会通过手动执行入口重复运行。</p>
<section v-else class="api-panel block" aria-label="API单任务开发">
<h3>API单任务开发 · {{task.title}}</h3>
<p>开发和只读自检使用同一模型、独立上下文。仅DeepSeek适配已实现；其他服务可先保存配置。</p>
<p v-if="error" role="alert">{{error}}</p><details v-if="diagnostic"><summary>详细原因（供排查）</summary>{{diagnostic}}</details><p v-if="message" role="status">{{message}}</p>
<p v-if="cleanup" role="alert">有{{cleanup}}项旧凭据清理失败已记录；不能视为旧Key已删除，请人工处理。</p>
<fieldset :disabled="locked"><legend>1. 模型服务</legend>
<label>已保存配置<select v-model="configId" @change="selectConfig"><option value="">新配置</option><option v-for="c in configs" :key="c.config.id" :value="c.config.id">{{c.config.provider}} / {{c.config.model}}{{c.adapter_ready?'':'（尚未适配）'}}</option></select></label>
<label>服务商<select v-model="provider" @change="preset"><option v-for="p in catalog" :key="p.provider" :value="p.provider">{{p.provider}}{{p.adapter_ready?'':'（尚未适配）'}}</option></select></label>
<label>Base URL<input v-model="base" placeholder="官方API地址或明确的兼容服务地址" /></label>
<label>模型名称<input v-model="model" placeholder="输入该服务实际支持的模型名称" /></label>
<p>当前配置Key：{{selectedConfig?.has_key?'已安全保存':'尚未设置'}}。{{selectedConfig&&!selectedConfig.adapter_ready?'该适配器尚未实现，不能执行。':''}}</p>
<label>API Key<input v-model="key" type="password" autocomplete="off" placeholder="仅临时输入，保存到本机凭据管理器" /></label>
<button @click="save">保存配置</button><button :disabled="!configId||provider!=='deepseek'" @click="connect">连接测试（将调用服务）</button>
</fieldset>
<button :disabled="!configId" @click="deleteKey">删除Key</button>
<fieldset :disabled="locked"><legend>2. 文件及固定测试</legend>
<label>项目内相对文件路径（每行一个，最多3个）<textarea v-model="paths" placeholder="例如 src/example.py" /></label>
<label>测试配方<select v-model="recipe"><option value="nova_vitest_api2">Nova前端API面板固定Vitest回归</option><option value="python_unittest">Python unittest：执行项目 tests/test_*.py</option><option value="content">只读内容断言（不是单元测试）</option></select></label>
<label>已安装的测试程序（Nova配方选Node，Python配方选Python）<input v-model="program" placeholder="项目以外的 node.exe 或 python.exe 完整路径" /></label><button @click="chooseProgram">选择程序</button>
<label v-if="recipe==='content'">文件必须包含<input v-model="contains" /></label>
<label><input v-model="allowRepair" type="checkbox" />预先允许同一范围内自动整改一次（最多4次模型请求，不扩大文件、测试、配置或费用条件）</label>
<button v-if="!run" :disabled="task.state!=='confirmed'||!selectedConfig?.adapter_ready" @click="prepare(false)">预览执行范围</button>
<button v-if="run&&['NEEDS_REMEDIATION','USER_REJECTED'].includes(run.phase)&&report?.repair_count===0" :disabled="!selectedConfig?.adapter_ready" @click="prepare(true)">准备一次整改（新范围需重新批准）</button>
</fieldset>
<section v-if="run"><h4>{{labels[run.phase]??'状态待核对'}}</h4><button @click="action(load)">刷新记录（只读）</button><button v-if="busy||['PREPARED','APPROVED','DEVELOP_STARTED','WRITE_INTENT','TEST_STARTED','REVIEW_STARTED'].includes(run.phase)" @click="cancel">取消执行</button>
<div v-if="run.phase==='PREPARED'&&plan">
<h4>3. 一次集中批准</h4>
<p>将向 {{plan.config.config.base_url}} 的 {{plan.config.config.model}} 发送任务需求、所选文件原文、修改与测试/自检证据。最多{{plan.allow_repair?4:2}}次请求，每次最多4096输出tokens、60秒，无自动网络重试；按服务商实际计费，不设置金额上限，取消后已发送请求仍可能计费。此批准仅适用于本任务及展示范围。</p>
<p>最多3个文件原位非原子写回，失败可能留下部分内容；保留原文及修改记录，不能自动还原用户文件。</p>
<ul><li v-for="f in plan.files" :key="f.path">允许修改：{{f.path}}</li></ul>
<div v-for="t in plan.tests" :key="t.specification.file"><p>测试：{{t.specification.recipe!=='content'?'项目测试，会执行项目代码，可能写文件、启动子进程或联网；不是沙箱，测试以当前用户权限运行，没有项目目录隔离。只批准可信项目及依赖，不自动安装或修复环境。':'只读内容断言，不代表项目测试。'}}</p><p>程序：{{t.specification.program}}；参数：{{t.argv.join(' ')}}；工作目录：{{t.cwd}}；每项10秒/输出4KiB。</p><p>仅传入列明环境：{{t.environment}}；程序hash不证明全部Python DLL、标准库或项目依赖可信。</p></div>
<details><summary>查看完整发送材料、原文和技术绑定</summary><pre>{{JSON.stringify(plan,null,2)}}</pre></details>
<label><input v-model="riskAccepted" type="checkbox" />我批准上述外发、费用、写入及测试影响{{plan.allow_repair?'，并允许同范围自动整改一次':''}}。</label><button :disabled="!riskAccepted" @click="approve">批准本任务范围</button></div>
<button v-if="run.phase==='APPROVED'" :disabled="busy" @click="execute">执行已批准任务</button>
<p>已使用整改次数：{{report?.repair_count??0}} / 1。{{report?.repair_count===1&&run.phase==='NEEDS_REMEDIATION'?'整改后仍未通过，需人工处理，不会继续循环。':''}}</p>
<p v-if="plan">当前批准对象：{{plan.root}}；{{plan.config.config.provider}} / {{plan.config.config.model}}；文件 {{plan.files.map((f:any)=>f.path).join('、')}}。执行只使用已批准计划。</p>
<h4>交付与实际证据</h4><p v-if="!run.summary.changes">尚无可展示修改。</p><details v-for="c in run.summary.changes??[]" :key="c.path" open><summary>{{c.path}}</summary><pre>{{c.candidate.diff}}</pre></details>
<p>实际测试：{{run.summary.tests??'未执行（NOT RUN）'}}</p><p>独立自检：{{run.summary.review??'未执行（NOT RUN）'}}</p>
<div v-if="run.phase==='AWAITING_USER'"><label>验收意见或修改要求<textarea v-model="feedback" /></label><button @click="decide(true)">验收通过并完成任务</button><button @click="decide(false)">{{plan?.allow_repair&&report?.repair_count===0?'提出修改并使用已授权的一次整改':'提出修改，暂不验收'}}</button></div>
<details><summary>结构化报告（供后续PM角色读取）</summary><pre>{{JSON.stringify(report,null,2)}}</pre></details>
</section>
</section>
</template>
<style scoped>
.api-panel{border:1px solid #a9bdb5;padding:16px;border-radius:8px}fieldset{margin:12px 0;padding:12px}label{display:block;margin:8px 0}input:not([type=checkbox]),select,textarea{display:block;width:min(100%,760px);padding:8px;box-sizing:border-box}textarea{min-height:72px}button{margin:4px;padding:8px}pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:360px;overflow:auto} [role=alert]{color:#a32323}
</style>
