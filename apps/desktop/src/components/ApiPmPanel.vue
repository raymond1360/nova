<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
const props=defineProps<{projectId:string}>()
const emit=defineEmits<{(event:'task-created',id:string):void;(event:'refresh',id:string):void}>()
const configs=ref<any[]>([]),catalog=ref<any[]>([]),record=ref<any>(null)
const pm=ref(''),execution=ref(''),goal=ref(''),paths=ref(''),program=ref(''),recipe=ref('python_unittest'),contains=ref(''),repair=ref(false)
const busy=ref(false),error=ref(''),detail=ref(''),risk=ref(false),network=ref(false),feedback=ref('')
const provider=ref('deepseek'),base=ref('https://api.deepseek.com/v1'),model=ref(''),key=ref(''),configId=ref('')
const phase=computed(()=>record.value?.phase??'NEW')
const labels:Record<string,string>={PREPARED:'请确认规划请求',PLANNING_STARTED:'产品经理正在形成任务',DRAFT:'请核对派单需求',READY:'请集中批准任务范围',APPROVED:'范围已批准，等待开始',RUNNING:'正在自动开发、测试和协调审核',AWAITING_USER:'技术检查通过，等待您最终验收',NEEDS_USER:'需要您决定下一步',STOPPED:'已停止，请查看原因',UNKNOWN:'结果待核对，不会自动重发',CANCELLED:'已取消'}
const editable=computed(()=>!record.value||['STOPPED','NEEDS_USER','CANCELLED','AWAITING_USER'].includes(phase.value))
const activeRole=computed(()=>{const r=record.value;const d=r?.decisions?.at(-1);if(d?.phase==='STARTED')return '产品经理';const p=r?.execution?.phase;return p==='REVIEW_STARTED'?'独立审核':p==='TEST_STARTED'?'固定测试':p==='DEVELOP_STARTED'?'开发':'协调器'})
// Derived from persisted records, so refresh/reopen does not erase the cause.
const executionSummary=computed(()=>record.value?.execution?.summary)
const failureCode=computed(()=>executionSummary.value?.error_code??record.value?.error_code??'')
const stoppedPhase=computed(()=>executionSummary.value?.stopped_phase??'')
const phaseNames:Record<string,string>={DEVELOP_STARTED:'开发请求',WRITE_INTENT:'文件写入',TEST_STARTED:'固定测试',REVIEW_STARTED:'独立审核请求'}
const failurePhase=computed(()=>phaseNames[stoppedPhase.value]??'未记录')
const transportFacts=computed(()=>{
 const s=executionSummary.value
 if(s){return stoppedPhase.value==='REVIEW_STARTED'?s.review_transport:s.develop_transport}
 return record.value?.decisions?.at(-1)?.transport
})
const responseProgress=computed(()=>{
 const f=transportFacts.value
 if(!f)return '传输进度：未记录（旧记录不补造历史值）'
 if(f.body_complete===true)return '响应体已完整收到'
 if(f.headers_received===true)return `已收到HTTP状态 ${f.http_status??'未记录'}，响应体尚未完整读取`
 return '尚未记录到响应头；不能据此确认上游是否收到请求'
})
const draftReasons:Record<string,string>={API_PM_DRAFT_SCHEMA_REJECTED:'派单格式版本不符合要求。',API_PM_DRAFT_ACTION_REJECTED:'派单动作不在允许范围。',API_PM_DRAFT_REASON_REJECTED:'派单说明超过允许长度。',API_PM_DRAFT_GOAL_REJECTED:'派单目标不能为空。',API_PM_DRAFT_REQUIREMENTS_REJECTED:'派单需求超过允许长度。',API_PM_DRAFT_ACCEPTANCE_REJECTED:'验收标准超过允许长度。',API_PM_DRAFT_FILES_REJECTED:'派单文件必须与本次选定文件完全一致，不允许扩围。',API_PM_DRAFT_REJECTED:'派单未通过合同校验；旧记录未记录具体字段原因。'}
const failureMessage=computed(()=>{
 if(failureCode.value==='API_PM_DRAFT_TEST_RECIPES_REJECTED'){
  const ids=(record.value?.packet?.tests??[]).map((t:any)=>t.specification?.recipe).filter((s:any)=>['python_unittest','content','nova_vitest_api2',''].includes(s)).map((s:string)=>s||'空字符串（兼容配方）')
  return `测试配方须使用已批准的${ids.length?ids.join('、'):'测试配方'}编号，不能返回程序路径、命令或说明文本。`
 }
 if(draftReasons[failureCode.value])return draftReasons[failureCode.value]
 if(failureCode.value==='API_TIMEOUT_UNKNOWN')return '请求在60秒传输期限内未取得完整结果，结果未知。'
 if(String(failureCode.value).includes('UNKNOWN'))return '执行结果未知，需要人工核对。'
 return '本次操作已停止，需要人工处理。'
})
async function refresh(){record.value=await invoke('api_pm_status',{projectId:props.projectId})}
async function action(f:()=>Promise<void>){error.value='';detail.value='';try{await f()}catch(e){error.value='操作已停止。请查看任务记录和详细原因；不会自动重试。';detail.value=String(e);await refresh().catch(()=>{})}}
async function prepare(){await action(async()=>{const list=paths.value.split(/\r?\n/).map(s=>s.trim()).filter(Boolean);if(!goal.value.trim()||!pm.value||!execution.value||!program.value||list.length<1||list.length>3){error.value='请填写目标、两个角色配置、1～3个文件和测试程序。超出范围请拆分任务。';return}record.value=await invoke('api_pm_prepare',{request:{project_id:props.projectId,goal:goal.value,pm_config_id:pm.value,execution_config_id:execution.value,paths:list,tests:[{recipe:recipe.value,program:program.value,file:list[0],contains:contains.value||'project tests'}],allow_repair:repair.value}});network.value=false})}
async function run(planning:boolean){if(busy.value)return;busy.value=true;const timer=setInterval(()=>{void refresh().catch(()=>{})},1500);await action(async()=>{record.value=await invoke('api_pm_run',{id:record.value.id,planningDigest:planning?record.value.digest:null,explicitNetwork:planning&&network.value})});clearInterval(timer);busy.value=false}
async function confirm(){await action(async()=>{record.value=await invoke('api_pm_confirm',{id:record.value.id});risk.value=false;if(record.value.task_id)emit('task-created',record.value.task_id)})}
async function approve(){await action(async()=>{record.value=await invoke('api_pm_approve',{id:record.value.id,digest:record.value.execution_approval_digest})})}
async function cancel(){await action(async()=>{await invoke('api_pm_cancel',{id:record.value.id});await refresh()})}
async function accept(accepted:boolean){await action(async()=>{if(!accepted&&!feedback.value.trim()){error.value='请说明需要修改的内容。';return}await invoke('api_only_user_decision',{id:record.value.run_id,accepted,note:feedback.value});await refresh();emit('refresh',record.value.task_id)})}
async function choose(){await action(async()=>{const p=await open({multiple:false,directory:false});if(typeof p==='string')program.value=p})}
async function save(){await action(async()=>{const secret=key.value;key.value='';const c:any=await invoke('api_only_save_config',{config:{id:configId.value||crypto.randomUUID(),provider:provider.value,base_url:base.value,model:model.value},key:secret||null});configId.value=c.config.id;configs.value=await invoke('api_only_configs')})}
async function deleteKey(){await action(async()=>{await invoke('api_only_delete_key',{id:configId.value});configs.value=await invoke('api_only_configs')})}
async function connect(){await action(async()=>{const r:any=await invoke('api_only_test_connection',{id:configId.value,explicitNetwork:true});error.value=r.reachable?'接口可达，开发能力尚未验证。':'连接未通过'})}
function selectConfig(){const c=configs.value.find(c=>c.config.id===configId.value)?.config;if(c){provider.value=c.provider;base.value=c.base_url;model.value=c.model}}
let alive=true
onMounted(()=>{void action(async()=>{const cs:any[]=await invoke('api_only_configs');if(!alive)return;configs.value=cs;catalog.value=await invoke('api_only_catalog');await refresh()})})
onUnmounted(()=>{alive=false})
</script>
<template>
<section class="pm-panel block" aria-label="产品经理自动协调">
<h3>产品经理自动协调 · 单项目单任务</h3>
<p>先允许产品经理读取选定材料形成派单，再集中批准执行。正常开发、测试、送审与已授权的一次整改自动交接；最终验收由您决定。</p>
<p v-if="error" role="alert">{{error}}</p><details v-if="detail"><summary>详细原因</summary>{{detail}}</details>
<details><summary>模型服务设置（Key仅保存到本机凭据管理器）</summary>
<select v-model="configId" @change="selectConfig"><option value="">新配置</option><option v-for="c in configs" :key="c.config.id" :value="c.config.id">{{c.config.provider}} / {{c.config.model}}</option></select>
<select v-model="provider" @change="base=catalog.find(c=>c.provider===provider)?.base_url??''"><option v-for="c in catalog" :key="c.provider" :value="c.provider">{{c.provider}}{{c.adapter_ready?'':'（尚未适配，不可执行）'}}</option></select>
<label>Base URL<input v-model="base" /></label><label>模型名称<input v-model="model" /></label><label>API Key<input v-model="key" type="password" autocomplete="off" /></label>
<button @click="save">保存模型配置</button><button :disabled="!configId" @click="deleteKey">删除所选Key</button><p>连接测试会显式请求服务，可能计入服务额度；可达不代表开发能力已验证。</p><button :disabled="!configId" @click="connect">连接测试（调用服务）</button>
</details>
<fieldset :disabled="!editable||busy"><legend>目标和允许范围</legend>
<label>产品经理模型<select v-model="pm"><option value="">请选择配置</option><option v-for="c in configs" :key="c.config.id" :value="c.config.id" :disabled="!c.adapter_ready">{{c.config.provider}} / {{c.config.model}}{{c.adapter_ready?'':'（未适配）'}}</option></select></label>
<label>开发及独立审核模型<select v-model="execution"><option value="">请选择配置</option><option v-for="c in configs" :key="c.config.id" :value="c.config.id" :disabled="!c.adapter_ready">{{c.config.provider}} / {{c.config.model}}</option></select></label>
<label>您希望完成什么<textarea v-model="goal" /></label><label>允许修改的相对文件路径（每行一个，最多3个）<textarea v-model="paths" /></label>
<label>固定测试<select v-model="recipe"><option value="python_unittest">Python unittest项目测试</option><option value="nova_vitest_api2">Nova前端API面板固定Vitest回归</option><option value="content">内容断言（不是单元测试）</option></select></label>
<label>项目以外的测试程序完整路径<input v-model="program" /></label><button @click="choose">选择测试程序</button><label v-if="recipe==='content'">必须包含<input v-model="contains" /></label>
<label><input v-model="repair" type="checkbox" />允许同范围、原期限内自动整改一次；不授权扩围或延长期限</label><button @click="prepare">预览规划请求</button>
</fieldset>
<section v-if="record"><h4>{{labels[phase]??phase}}</h4><p v-if="busy">当前负责：{{activeRole}}。关闭进程或休眠可能中断；未知阶段不会自动重放。</p><p v-else-if="['RUNNING','PLANNING_STARTED','UNKNOWN'].includes(phase)">没有可证明的后台活性，请人工核对；不会因刷新重新发送。</p>
<button @click="action(refresh)">刷新协调记录（只读）</button><button v-if="['PREPARED','DRAFT','READY','APPROVED','RUNNING','PLANNING_STARTED'].includes(phase)" @click="cancel">取消协调</button>
<div v-if="phase==='PREPARED'"><p>规划将向 {{record.packet.pm.config.base_url}} / {{record.packet.pm.config.model}} 发送目标、选定文件完整原文及固定测试材料。仅1次请求，60秒、输入128KiB、输出4096 tokens，无自动重试，可能按服务商规则计费；不等于批准执行。</p><details><summary>查看准确规划外发材料</summary><pre>{{record.packet}}</pre></details><label><input v-model="network" type="checkbox" />我允许这一次规划请求及所列材料外发</label><button :disabled="!network||busy" @click="run(true)">允许产品经理形成派单</button></div>
<div v-if="record.draft"><h4>产品经理派单</h4><p>目标：{{record.draft.goal}}</p><p>需求：{{record.draft.requirements}}</p><p>验收标准：{{record.draft.acceptance}}</p><p>说明或澄清：{{record.draft.reason}}</p><button v-if="phase==='DRAFT'" @click="confirm">确认需求并预览执行范围</button></div>
<div v-if="phase==='READY'"><h4>一次集中批准</h4><p>PM使用 {{record.packet.pm.config.provider}} / {{record.packet.pm.config.model}}；开发和只读审核使用 {{record.packet.execution.config.provider}} / {{record.packet.execution.config.model}}，上下文独立。</p><p>PM合计最多5次（包括已发的规划1次）；开发/审核合计最多4次，整体最多9次。每次60秒、输入128KiB、输出4096 tokens，无网络自动重试；不设金额上限，token上限不等于费用硬上限。已发送调用取消后仍可能计费。</p><p>将外发需求、所选源码、实际差异、测试输出和审核问题。仅在当前所选文件、固定测试、配置及原期限内{{record.packet.request.allow_repair?'允许一次整改':'不允许自动整改'}}。期限：{{new Date(record.packet.expires_utc*1000).toLocaleString()}}。</p><p>原位写回不是原子操作，失败可能留下部分文件；保留原文，不自动恢复用户文件。测试执行可信项目代码，可能写文件、创建子进程及联网；不是沙箱，不自动安装依赖。</p><ul><li v-for="f in record.plan.files" :key="f.path">{{f.path}}</li></ul><div v-for="t in record.plan.tests" :key="t.specification.file"><p>固定程序：{{t.specification.program}}；参数：{{t.argv}}；目录：{{t.cwd}}；环境：{{t.environment}}；10秒/输出4KiB。程序hash不能证明全部依赖可信。</p></div><details><summary>完整执行材料与原文件</summary><pre>{{record.plan}}</pre></details><label><input v-model="risk" type="checkbox" />我批准所列外发、费用、写入和固定测试范围及一次整改设置</label><button :disabled="!risk" @click="approve">批准协调范围</button></div>
<button v-if="phase==='APPROVED'" :disabled="busy" @click="run(false)">开始自动协调</button>
<p>PM请求已预约：{{record.pm_requests_reserved}} / 5；开发和审核：{{record.execution_requests_reserved??0}} / 4；合计 {{record.overall_requests_reserved??record.pm_requests_reserved}} / 9。上述是请求预约数，不是实际计费次数。开发、测试和自检以实际工具收据为准；未执行不算通过。</p>
<div v-if="record.execution"><p>执行阶段：{{record.execution.phase}}；已用整改：{{record.report?.repair_count??0}} / 1</p><details v-for="c in record.execution.summary.changes??[]" :key="c.path" open><summary>{{c.path}}</summary><pre>{{c.candidate.diff}}</pre></details><p>实际测试：{{record.execution.summary.tests??'NOT RUN（未执行）'}}</p><p>独立审核：{{record.execution.summary.review??'NOT RUN（未执行）'}}</p></div>
<div v-if="phase==='AWAITING_USER'&&record.execution?.phase==='AWAITING_USER'"><label>最终验收意见<textarea v-model="feedback" /></label><button @click="accept(true)">用户验收通过</button><button @click="accept(false)">提出修改，交人工决定</button></div>
<p v-if="record.execution?.phase==='ACCEPTED'">用户已验收，任务完成。</p><p v-if="record.decisions?.at(-1)?.output?.reason">产品经理说明：{{record.decisions.at(-1).output.reason}}</p>
<section v-if="failureCode" role="alert" aria-label="持久错误说明">
<p>{{failureMessage}} 停止阶段：{{failurePhase}}。不会自动重发。</p>
<p>{{responseProgress}}</p>
<p>尝试开始：{{transportFacts?.attempt_started_utc??'未记录'}}；耗时：{{transportFacts?.elapsed_ms??'未记录'}} 毫秒；已读响应体：{{transportFacts?.body_bytes_read??'未记录'}} 字节。</p>
<p>测试：{{executionSummary?.tests==='NOT RUN'?'未运行':executionSummary?.tests?'请查看实际测试证据':'未记录'}}；审核：{{executionSummary?.review==='NOT RUN'?'未运行':executionSummary?.review?'请查看独立审核证据':'未记录'}}。</p>
<p>文件是否改动须核对修改记录与当前文件，不能仅凭超时判断。请保留现场交由负责人核对，不要重复执行、清除未知状态或另建计划绕过。</p>
<p v-if="executionSummary?.transport_recording==='INCOMPLETE'">部分传输诊断未能持久保存；已有字段不能代表完整历史。</p>
</section>
<p v-else-if="phase==='NEEDS_USER'">需要人工处理。请查看下方决策原因；不会自动扩围或重复请求。</p>
<details><summary>交付汇总、PM决策和技术绑定详情</summary><pre>{{record}}</pre></details>
</section></section>
</template>
<style scoped>
.pm-panel{border:1px solid #a9bdb5;padding:16px;border-radius:8px;margin:12px 0}label{display:block;margin:8px 0}textarea,input:not([type=checkbox]),select{display:block;width:min(100%,760px);padding:8px;box-sizing:border-box}button{margin:4px;padding:8px}pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:360px;overflow:auto}[role=alert]{color:#a32323}
</style>
