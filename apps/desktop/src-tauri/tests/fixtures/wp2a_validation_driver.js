// Test-only DOM driver inside the REAL Tauri WebView. No invoke mocks or store writes.
// DOM events drive the original Vue forms/actions; explicit negative API probes use real IPC.
addEventListener('DOMContentLoaded', () => { setTimeout(async () => {
  const invoke = (name,args={}) => window.__TAURI_INTERNALS__.invoke(name,args);
  const ctl = (action,payload={}) => invoke('validation_control',{action,payload});
  const wait = ms => new Promise(r=>setTimeout(r,ms));
  const visible = e => e && e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
  const buttons=()=>[...document.querySelectorAll('button')].filter(visible);
  const button = label => buttons().find(e=>e.textContent.trim()===label);
  const text=()=>document.body.innerText;
  const until = async (fn,label) => {for(let i=0;i<150;i++){const v=await fn();if(v)return v;await wait(100);}throw Error('timeout: '+label);};
  const click = async el => {if(!el)throw Error('DOM target missing');el.scrollIntoView({block:'center'});el.click();await wait(180);};
  const type = (selector,value) => {const e=document.querySelector(selector);if(!e)throw Error('input missing '+selector);e.value=value;e.dispatchEvent(new Event('input',{bubbles:true}));};
  const assert=(value,label)=>{if(!value)throw Error('assertion: '+label);};
  const snap=()=>ctl('snapshot');
  const results=[];
  const checkpoint=async(name,details={},selector)=>{
    if(selector)document.querySelector(selector)?.scrollIntoView({block:'center'});
    await wait(250);
    await ctl('checkpoint',{name,details,dom:text()});
    for(let i=0;i<900;i++){if(await ctl('ack',{name}))return;await wait(200);}throw Error('screenshot ack timeout '+name);
  };
  const run=async(name,fn)=>{try{await fn();results.push({name,passed:true});}catch(e){results.push({name,passed:false,error:String(e)});await checkpoint(name+'-FAILED',{error:String(e)});}};
  let fixture;
  const fixtureList=async()=>{const db=await snap();return db.tasks;};
  const select = async prefix => {await click(await until(()=>buttons().find(e=>e.textContent.trim().startsWith(prefix)),'task '+prefix));await wait(550);};
  const stateOf=async id=>(await snap()).tasks.find(t=>t.id===id)?.state;
  const openSummary=async label=>{const s=[...document.querySelectorAll('summary')].find(e=>e.textContent.includes(label));if(s&&!s.parentElement.open)await click(s);};
  const refreshProject=async()=>{await click(button('刷新项目列表'));await click(await until(()=>buttons().find(e=>e.textContent.includes('WP2A ISOLATED FIXTURE —')),'project list'));await wait(500);};
  const start=async prefix=>{await select(prefix);await click(button('检查 Codex / Claude CLI 就绪状态'));await until(()=>button('开始任务')&&!button('开始任务').disabled,'fake CLI readiness');await click(button('开始任务'));};
  const expectRejected=async(name,args)=>{let rejected=false;try{await invoke(name,args);}catch{rejected=true;}assert(rejected,name+' must reject');};
  try {
    await until(()=>window.__TAURI_INTERNALS__&&button('项目任务'),'frontend mount');
    await click(button('项目任务'));await until(()=>document.querySelector('.core-loop-view'),'real CoreLoopView');
    await refreshProject();fixture=await fixtureList();
    await checkpoint('00-isolation',{actualTauri:!!window.__TAURI_INTERNALS__,fixtureTasks:fixture.length},'.core-loop-view');
    let created;
    await run('01-confirmation-snapshot',async()=>{
      type('input[placeholder="任务标题"]','UI CREATED REQUIREMENT');type('textarea[placeholder="目标 / 需求描述"]','SCENARIO:complete\nUI export\nUI export');type('textarea[placeholder="本次范围内"]','CSV');type('textarea[placeholder="明确不包含"]','cloud');type('textarea[placeholder="完成标准"]','UTF8');
      await click(button('创建任务'));created=(await snap()).tasks.find(t=>t.title==='UI CREATED REQUIREMENT');assert(created,'created by DOM');
      await expectRejected('generate_requirement_snapshot',{projectId:created.project_id,taskId:created.id});
      assert((await snap()).confirmed_requirement_items.filter(i=>i.task_id===created.id).length===0,'Draft zero items');
      await click(button('我已确认需求，继续'));await until(async()=>await stateOf(created.id)==='confirmed','confirmed');
      const items=(await snap()).confirmed_requirement_items.filter(i=>i.task_id===created.id);assert(items.length===6,'automatic six items including duplicate lines');
      await checkpoint('01-confirmation-snapshot',{task:created.id,count:items.length},'.core-loop-view');
    });
    await run('02-explicit-backfill',async()=>{
      const t=fixture.find(t=>t.title.startsWith('05 '));await select('05 SNAPSHOT');await openSummary('查看原需求并确认沿用');
      const details=[...document.querySelectorAll('details')].find(d=>d.textContent.includes('确认沿用，生成需求清单'));
      assert(details.querySelectorAll('.requirement-text').length===4,'four read-only original fields');assert(!details.querySelector('textarea,input:not([readonly])'),'no editable requirement fields');
      await click(button('确认沿用，生成需求清单'));await until(()=>text().includes('需求清单已确认'),'backfill result');
      const before=(await snap()).confirmed_requirement_items.filter(i=>i.task_id===t.id);assert(before.length===6,'six backfilled items');
      await click(button('确认沿用，生成需求清单'));await wait(300);const after=(await snap()).confirmed_requirement_items.filter(i=>i.task_id===t.id);assert(JSON.stringify(before)===JSON.stringify(after),'repeat preserves IDs and count');
      await checkpoint('02-explicit-backfill',{task:t.id,count:after.length},'.requirement-text');
    });
    const normal=fixture.find(t=>t.title.startsWith('01 '));
    await run('03-fake-cli-summary-precheck',async()=>{
      await start('01 NORMAL');await until(async()=>await stateOf(normal.id)==='awaiting_user_acceptance','fake development completes');await refreshProject();await select('01 NORMAL');await openSummary('开发 AI 自检摘要');
      assert(text().includes('自评已完成'),'self-check summary');assert(!button('通过验收').disabled,'first acceptance enabled');
      const p=await invoke('get_completion_precheck',{projectId:normal.project_id,taskId:normal.id});assert(p.can_submit_acceptance&&!p.ok,'submit vs direct gate');
      await checkpoint('03-fake-cli-summary-precheck',{precheck:p},'.accept-block');
    });
    await run('04-first-user-acceptance',async()=>{
      await click(button('通过验收'));await until(async()=>await stateOf(normal.id)==='completed','completed');
      const db=await snap();assert(db.user_acceptance.filter(a=>a.task_id===normal.id&&a.accepted===1).length===1,'one actual acceptance');
      await expectRejected('generate_requirement_snapshot',{projectId:normal.project_id,taskId:normal.id});
      await checkpoint('04-first-user-acceptance',{task:normal.id,state:await stateOf(normal.id)},'.accept-block');
    });
    await run('05-self-check-rejections',async()=>{
      for(const prefix of ['02 MISSING','03 INVALID','04 INCOMPLETE']){
        const t=fixture.find(t=>t.title.startsWith(prefix));await start(prefix);await until(async()=>await stateOf(t.id)==='awaiting_user_acceptance',prefix);await refreshProject();await select(prefix);await openSummary('开发 AI 自检摘要');
        assert(button('通过验收').disabled,'accept disabled '+prefix);assert(document.querySelector('.precheck-banner'),'clear reason '+prefix);
        const view=[...document.querySelectorAll('.document-list li')].find(e=>e.querySelector('span')?.textContent.trim()==='最终结果')?.querySelector('button');await click(view);await until(()=>document.querySelector('.document-content pre')?.textContent.includes(t.id),'actual final result document visible');
        const count=(await snap()).task_state_events.length;await expectRejected('transition_task',{taskId:t.id,toState:'completed',reason:'ISOLATED negative IPC probe'});assert((await snap()).task_state_events.length===count,'no false event');
        await checkpoint('05-'+prefix.split(' ')[0],{state:await stateOf(t.id),reason:document.querySelector('.precheck-banner')?.textContent},'.precheck-banner');
      }
    });
    await run('06-case-c-recovery',async()=>{
      const t=fixture.find(t=>t.title.startsWith('06 '));await select('06 CASE');await until(()=>button('确认当前无需构建'),'Case C visible');
      const before=await snap();await click(button('确认当前无需构建'));await until(async()=>await stateOf(t.id)==='queued','Case C queued');await wait(700);const after=await snap();
      assert(before.no_build_confirmations.filter(r=>r.task_id===t.id).length===after.no_build_confirmations.filter(r=>r.task_id===t.id).length,'no duplicate confirmation');
      const action=rows=>rows.find(a=>a.task_id===t.id&&a.action_type==='build_execution');assert(action(before.task_actions).attempt_number===action(after.task_actions).attempt_number,'no second reset');
      assert(button('开始任务'),'queued has explicit start button');await checkpoint('06-case-c-recovery',{task:t.id,state:await stateOf(t.id),attempt:action(after.task_actions).attempt_number},'.core-loop-view');
    });
    await run('07-stale-and-read-errors',async()=>{
      const t=fixture.find(t=>t.title.startsWith('09 '));
      for(const command of ['get_completion_precheck','list_task_self_checks']){
        await ctl('delay',{command,task:t.id,ms:1400});await select('09 GENERATION');await select('03 INVALID');await wait(1700);assert(button('通过验收').disabled,'old success must not enable invalid task');assert(document.querySelector('.precheck-banner')?.textContent.includes('无效'),'invalid reason retained');assert(!text().includes('自评已完成'),'old successful self-check summary rejected');await checkpoint('07-late-'+command,{task:'03 INVALID'},'.accept-block');
      }

      await ctl('break_reads',{});await select('03 INVALID');await select('09 GENERATION');await until(()=>text().includes('完工预检读取失败')&&text().includes('自检记录读取失败'),'actual SQLite read error displayed');await checkpoint('07-read-failure',{task:t.id},'.accept-block');
      await ctl('break_reads',{restore:true});await select('03 INVALID');assert(!text().includes('读取失败'),'old errors cleared on switch');
      await ctl('delay',{command:'get_completion_precheck',task:t.id,ms:1400});await select('09 GENERATION');await ctl('advance_generation',{task:t.id});await refreshProject();await select('09 GENERATION');await wait(1600);assert((await snap()).tasks.find(r=>r.id===t.id).result_generation===2,'new generation');assert(button('通过验收').disabled,'old generation precheck not accepted');
      await select('03 INVALID');await checkpoint('07-stale-switch',{task:'03 INVALID'},'.precheck-banner');
    });
    await run('08-old-attempt-history',async()=>{
      const t=fixture.find(t=>t.title.startsWith('07 '));await select('07 OLD');await openSummary('查看完整时间线');
      const overview=await invoke('get_task_evidence_overview',{projectId:t.project_id,taskId:t.id});assert(overview.tests.every(r=>!r.is_current_attempt),'no current verification');assert(text().includes('（历史）'),'history label visible');
      await checkpoint('08-old-attempt-history',{currentAttempt:false},'.timeline-detail');
    });
    await run('09-frozen-history',async()=>{
      const t=fixture.find(t=>t.title.startsWith('08 '));await select('08 FROZEN');assert(button('通过验收').disabled,'frozen cannot accept');const before=await snap();
      await expectRejected('accept_task_result',{projectId:t.project_id,taskId:t.id,accepted:true,note:'ISOLATED frozen probe'});await expectRejected('start_core_loop',{taskId:t.id});assert((await snap()).user_acceptance.length===before.user_acceptance.length,'no frozen acceptance write');
      await click([...document.querySelectorAll('button')].find(e=>e.textContent.trim()==='查看'));await until(()=>document.querySelector('.document-content pre')?.textContent.includes(t.id),'frozen document visible');await checkpoint('09-frozen-view',{task:t.id},'.precheck-banner');
      // There is no generic Cancel button in this awaiting state: explicit frontend
      // driver IPC uses the EXISTING production transition wrapper, not a fixture mutation.
      await invoke('transition_task',{taskId:t.id,toState:'cancelled',reason:'ISOLATED legal cancellation IPC'});await refreshProject();await select('08 FROZEN');assert(await stateOf(t.id)==='cancelled','legal cancellation');await checkpoint('09-frozen-cancel',{state:'cancelled'},'.core-loop-view');
    });
    await ctl('finish',{completed:true,results});await checkpoint('99-finished',{results},'.core-loop-view');
  } catch(e) {await ctl('finish',{completed:false,error:String(e),results});}
},700); });
