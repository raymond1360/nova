export type Language = 'zh' | 'en'
export const copy = {
  zh: {
    title: '任务只读工作台', preview: '新版只读预览', back: '返回原界面', projects: '项目', tasks: '任务记录',
    chooseProject: '选择现有项目', chooseTask: '选择任务查看记录', emptyProjects: '暂无已登记项目', emptyTasks: '此项目暂无任务记录',
    intro: '查看已有工作与证据。这里不发送请求、不启动任务，也不确认完成。',
    recorded: '记录阶段', unknown: '尚无可确认的记录', liveUnknown: '记录不证明进程当前存活',
    timeline: '真实时间线', emptyTimeline: '暂无时间线记录', readAt: '上次成功读取', eventAt: '事件时间',
    loading: '正在读取记录…', readError: '读取失败，请重新读取已有记录。', stale: '上次快照已过期，当前状态尚未确认',
    refresh: '刷新记录', evidence: '证据详情', close: '收起证据', emptyEvidence: '暂无证据记录',
    commands: '命令与脱敏输出', tests: '测试证据', builds: '构建证据', changes: '文件变更', acceptance: '用户验收历史',
    generation: '结果代次', round: '执行轮次', historical: '历史记录', adopted: '已采用', notAdopted: '未标记为已采用',
    currentAttempt: '当前尝试', otherAttempt: '其他尝试', snapshotUnknown: '当前文件快照未知',
    draft: '本窗口草稿，未发送、未确认', draftHint: '仅在本窗口保留；Enter 换行，不发送。关闭窗口后不承诺保留。',
    placeholder: '记下之后想修改的内容…', cacheFailed: '缓存不可用：输入仍在内存，刷新或关闭窗口可能丢失。',
    disabled: '发送与执行未开放', language: 'English', theme: '暖纸', green: '绿意', readonly: '只读 · 未启用执行',
    exit: '退出码', approved: '已接受', rejected: '未接受', showSidebar: '项目与任务', needsAction: '需要用户处理',
  },
  en: {
    title: 'Task record workspace', preview: 'Read-only preview', back: 'Back to original UI', projects: 'Projects', tasks: 'Task records',
    chooseProject: 'Select an existing project', chooseTask: 'Choose a task to read', emptyProjects: 'No registered projects', emptyTasks: 'No task records in this project',
    intro: 'Read existing work and evidence. This view cannot send, start tasks or accept completion.',
    recorded: 'Recorded stage', unknown: 'No confirmed record yet', liveUnknown: 'A record does not prove the process is alive',
    timeline: 'Recorded timeline', emptyTimeline: 'No timeline records', readAt: 'Last successful read', eventAt: 'Event time',
    loading: 'Reading records…', readError: 'Could not read records. Refresh to check again.', stale: 'Previous snapshot is stale; current state is unconfirmed',
    refresh: 'Refresh records', evidence: 'Evidence details', close: 'Close evidence', emptyEvidence: 'No evidence records',
    commands: 'Commands and redacted output', tests: 'Test evidence', builds: 'Build evidence', changes: 'File changes', acceptance: 'User acceptance history',
    generation: 'Generation', round: 'Execution round', historical: 'Historical', adopted: 'Adopted', notAdopted: 'Not marked adopted',
    currentAttempt: 'Current attempt', otherAttempt: 'Other attempt', snapshotUnknown: 'Current file snapshot unknown',
    draft: 'Window draft, not sent or confirmed', draftHint: 'Kept in this window only. Enter adds a line; it does not send. Closing the window may discard it.',
    placeholder: 'Note what you might want to change later…', cacheFailed: 'Cache unavailable: input remains in memory, but may be lost on reload or close.',
    disabled: 'Sending and execution unavailable', language: '中文', theme: 'Warm paper', green: 'Green', readonly: 'Read only · execution disabled',
    exit: 'Exit code', approved: 'Accepted', rejected: 'Not accepted', showSidebar: 'Projects and tasks', needsAction: 'Needs user attention',
  },
} as const
const stages: Record<string, [string, string]> = {
  draft: ['草稿', 'Draft'], awaiting_confirmation: ['待确认', 'Awaiting confirmation'], confirmed: ['已确认', 'Confirmed'],
  queued: ['已排队', 'Queued'], running: ['开发阶段记录', 'Development recorded'], verifying: ['验证阶段记录', 'Verification recorded'],
  safe_fixing: ['修复阶段记录', 'Repair recorded'], preview_ready: ['待查看', 'Ready to review'], awaiting_user_acceptance: ['待用户验收', 'Awaiting acceptance'],
  needs_user_action: ['需要处理', 'Needs attention'], interrupted: ['已中断', 'Interrupted'], failed: ['失败', 'Failed'],
  cancelled: ['已取消', 'Cancelled'], completed: ['已完成记录', 'Completion recorded'],
}
export const stage = (code: string, language: Language) => stages[code]?.[language === 'zh' ? 0 : 1] ?? code
export const tokens = {
  green: { '--ro-bg': '#f7f8f7', '--ro-side': '#edf1ee', '--ro-accent': '#17663f', '--ro-soft': '#e8f4ec', '--ro-border': '#dce4de' },
  paper: { '--ro-bg': '#fcfbf9', '--ro-side': '#f8f6f1', '--ro-accent': '#994627', '--ro-soft': '#f8eee8', '--ro-border': '#e6e1d8' },
}
