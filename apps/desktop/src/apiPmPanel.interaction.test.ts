import {beforeEach,afterEach,describe,it,expect,vi} from 'vitest'
import {defineComponent,h} from 'vue'
import {createPinia} from 'pinia'
vi.mock('@tauri-apps/api/core',()=>({invoke:vi.fn()}))
vi.mock('@tauri-apps/plugin-dialog',()=>({open:vi.fn()}))
import {invoke} from '@tauri-apps/api/core'
import Panel from './components/ApiPmPanel.vue'
import {renderComponent,settle,all,click,button,TestNode} from './testSupport/interactionRenderer'
const ipc=vi.mocked(invoke)
let mounted:ReturnType<typeof renderComponent>|undefined
let phase='PREPARED',accepted=false
const config={id:'pm-config',provider:'deepseek',base_url:'https://api.deepseek.com/v1',model:'fake-pm'}
const packet={request:{allow_repair:true},pm:{config},execution:{config:{...config,id:'exec-config',model:'fake-developer'}},expires_utc:2000000000}
function record(){return {id:'packet',digest:'digest',phase,packet,execution_approval_digest:'bound-user-digest',pm_requests_reserved:phase==='PREPARED'?0:3,decisions:[],draft:phase==='PREPARED'?null:{goal:'goal',requirements:'requirements',acceptance:'actual tests pass',reason:'bounded'},plan:{files:[{path:'note.txt'}],tests:[]},run_id:'run',execution:['AWAITING_USER','ACCEPTED'].includes(phase)?{phase,summary:{tests:[{status:'PASS',counts:{executed:1}}],review:{verdict:'PASS'}}}:null,report:{repair_count:0}}}
function route(){ipc.mockImplementation(async(command,args:any)=>{switch(command){case 'api_only_configs':return [{config,adapter_ready:true},{config:{...config,id:'exec-config'},adapter_ready:true}] as any;case 'api_only_catalog':return [{provider:'deepseek',adapter_ready:true,base_url:config.base_url}] as any;case 'api_pm_status':return record() as any;case 'api_pm_run':expect(args.planningDigest===null||args.explicitNetwork===true).toBe(true);phase=args.planningDigest?'DRAFT':'AWAITING_USER';return record() as any;case 'api_pm_confirm':phase='READY';return record() as any;case 'api_pm_approve':phase='APPROVED';return record() as any;case 'api_pm_cancel':phase='CANCELLED';return undefined as any;case 'api_only_user_decision':accepted=args.accepted;phase=accepted?'ACCEPTED':'USER_REJECTED';return {} as any;default:throw Error('Unexpected '+command)}})}
async function mount(){mounted=renderComponent(defineComponent({setup:()=>()=>h(Panel,{projectId:'project'})}),createPinia());await settle();return mounted.root}
async function check(root:TestNode,last=false){const list=all(root,n=>n.kind==='input'&&n.props.type==='checkbox');const c=last?list.at(-1)!:list.at(-1)!;c.checked=true;c.dispatchEvent(new Event('change'));await settle()}
beforeEach(()=>{vi.clearAllMocks();phase='PREPARED';accepted=false;vi.stubGlobal('document',{activeElement:null});vi.stubGlobal('Document',class {});vi.stubGlobal('ShadowRoot',class {});route()})
afterEach(()=>{mounted?.unmount();mounted=undefined;vi.unstubAllGlobals()})
describe('API PM concrete panel',()=>{
 it('mount refresh and reopen only read records',async()=>{const root=await mount();await click(button(root,'刷新协调记录（只读）'));mounted?.unmount();await mount();expect(ipc.mock.calls.every(c=>['api_only_configs','api_only_catalog','api_pm_status'].includes(c[0]))).toBe(true)})
 it('planning consent is separate from execution approval and automatic handoffs use one coordinator call',async()=>{const root=await mount();expect(button(root,'允许产品经理形成派单').props.disabled).toBe(true);await check(root);await click(button(root,'允许产品经理形成派单'));expect(ipc.mock.calls.some(c=>c[0]==='api_pm_approve')).toBe(false);await click(button(root,'确认需求并预览执行范围'));expect(root.textContent).toContain('整体最多9次');expect(root.textContent).toContain('fake-pm');expect(root.textContent).toContain('fake-developer');expect(button(root,'批准协调范围').props.disabled).toBe(true);await check(root,true);await click(button(root,'批准协调范围'));expect(ipc.mock.calls.filter(c=>c[0]==='api_pm_run')).toHaveLength(1);await click(button(root,'开始自动协调'));expect(ipc.mock.calls.filter(c=>c[0]==='api_pm_run')).toHaveLength(2);expect(root.textContent).toContain('等待您最终验收');expect(accepted).toBe(false);await click(button(root,'用户验收通过'));expect(accepted).toBe(true)})
 it('unknown/reopened running states never expose replay',async()=>{for(const s of ['UNKNOWN','RUNNING','PLANNING_STARTED']){phase=s;const root=await mount();expect(all(root,n=>n.kind==='button'&&/开始自动协调|允许产品经理形成派单/.test(n.textContent))).toHaveLength(0);mounted?.unmount()}expect(ipc.mock.calls.some(c=>c[0]==='api_pm_run')).toBe(false)})
 it('pending cancel is not a model call',async()=>{const root=await mount();await click(button(root,'取消协调'));expect(root.textContent).toContain('已取消');expect(ipc.mock.calls.some(c=>c[0]==='api_pm_run')).toBe(false)})
})

function readonlyFailureRoute(stored:any){ipc.mockImplementation(async(command)=>{switch(command){case 'api_only_configs':return [] as any;case 'api_only_catalog':return [] as any;case 'api_pm_status':return structuredClone(stored);default:throw Error('Unexpected '+command)}})}
function errorPanel(root:TestNode){return all(root,n=>n.props['aria-label']==='持久错误说明')[0]!}
describe('persisted timeout explanation',()=>{
 it('legacy persisted error stays visible after refresh/reopen with zero new requests',async()=>{
  const stored={...record(),phase:'UNKNOWN',error_code:'API_PM_EXECUTION_UNKNOWN',execution:{phase:'UNKNOWN',summary:{error_code:'API_TIMEOUT_UNKNOWN',stopped_phase:'DEVELOP_STARTED',tests:'NOT RUN',review:'NOT RUN'}},pm_requests_reserved:1,execution_requests_reserved:1,overall_requests_reserved:2}
  readonlyFailureRoute(stored)
  let root=await mount()
  for(let i=0;i<2;i++){
   const text=errorPanel(root).textContent
   expect(text).toContain('60秒');expect(text).toContain('结果未知');expect(text).toContain('停止阶段：开发请求')
   expect(text).toContain('测试：未运行');expect(text).toContain('审核：未运行');expect(text).toContain('不会自动重发')
   expect(text).toContain('未记录');expect(text).not.toContain('文件未改')
   await click(button(root,'刷新协调记录（只读）'));mounted?.unmount();root=await mount()
  }
  expect(root.textContent).toContain('请求预约数，不是实际计费次数')
  expect(ipc.mock.calls.every(c=>['api_only_configs','api_only_catalog','api_pm_status'].includes(c[0]))).toBe(true)
 })
 it('renders only observed header/body boundaries, including completed response',async()=>{
  for(const [facts,message]of [
   [{headers_received:false,http_status:null,body_complete:false,body_bytes_read:0,elapsed_ms:60000},'尚未记录到响应头'],
   [{headers_received:true,http_status:200,body_complete:false,body_bytes_read:17,elapsed_ms:60000},'已收到HTTP状态 200，响应体尚未完整读取'],
   [{headers_received:true,http_status:200,body_complete:true,body_bytes_read:128,elapsed_ms:12},'响应体已完整收到'],
  ] as const){
   readonlyFailureRoute({...record(),phase:'UNKNOWN',execution:{phase:'UNKNOWN',summary:{error_code:'API_TIMEOUT_UNKNOWN',stopped_phase:'REVIEW_STARTED',review_transport:facts,tests:[{status:'PASS'}],review:'NOT RUN'}}})
   const root=await mount();expect(errorPanel(root).textContent).toContain(message)
   expect(errorPanel(root).textContent).toContain('独立审核请求');mounted?.unmount()
  }
  expect(ipc.mock.calls.every(c=>['api_only_configs','api_only_catalog','api_pm_status'].includes(c[0]))).toBe(true)
 })
})
