import { useEffect, useMemo, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  AlertCircle,
  CalendarClock,
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  Clock3,
  Database,
  ListFilter,
  RefreshCw,
  RotateCw,
  Server,
  Sparkles,
  WifiOff,
  X,
} from 'lucide-react'
import {
  type CreateAction,
  type EventInput,
  type EventPreview,
  type EventView,
  formFromBootstrap,
  formToEventInput,
  getFormErrors,
  trackerApi,
  TrackerApiError,
  type FormState,
} from './api'
import './App.css'

type Notice = {
  tone: 'success' | 'warning' | 'danger' | 'info'
  title: string
  message: string
}

type FilterState = {
  status: '' | 'pending' | 'synced'
  eventDate: string
  calendarName: string
}

const emptyForm: FormState = {
  calendar_name: '',
  event_date: '',
  start_time: '',
  title: '',
  first_item: '',
  last_item: '',
}

const defaultFilters: FilterState = {
  status: 'synced',
  eventDate: '',
  calendarName: '',
}

function getErrorMessage(error: unknown, fallback = '请求未完成，请稍后重试。') {
  if (error instanceof TrackerApiError) return error.message
  return fallback
}

function statusLabel(event: EventView) {
  if (event.sync_status === 'synced') return '已同步'
  if (event.last_sync_error) return '同步失败，已保存'
  return '待同步'
}

function statusTone(event: EventView) {
  if (event.sync_status === 'synced') return 'synced'
  if (event.last_sync_error) return 'failed'
  return 'pending'
}

function outcomeNotice(
  result: { event: EventView; sync: { status: string; action?: string | null; error?: { code: string; message: string } | null } },
): Notice {
  if (result.sync.status === 'succeeded' || result.sync.status === 'already_synced') {
    const action = result.sync.action === 'existing' ? '已确认 Calendar 中已有对应事件' : '已创建 Calendar 事件'
    return {
      tone: 'success',
      title: '记录已保存并同步',
      message: `${action} · ${result.event.title} · ${result.event.id}`,
    }
  }
  if (result.sync.error?.code === 'sync_state_unknown') {
    return {
      tone: 'danger',
      title: '记录已保存，但同步结果未知',
      message: `请不要重复创建，先在列表中确认 ${result.event.id}，再按 ID 重试。`,
    }
  }
  if (result.sync.status === 'failed') {
    return {
      tone: 'warning',
      title: '记录已保存，等待同步',
      message: result.sync.error?.message ?? 'Calendar 暂时不可用，可稍后重试。',
    }
  }
  return {
    tone: 'info',
    title: '记录已保存',
    message: '事件已写入本地数据库，可从待同步列表继续处理。',
  }
}

function formatDateLabel(value: string) {
  if (!value) return ''
  const parts = value.split('-')
  return parts.length === 3 ? `${parts[1]}/${parts[2]}` : value
}

function App() {
  const queryClient = useQueryClient()
  const [form, setForm] = useState<FormState>(emptyForm)
  const [formInitialized, setFormInitialized] = useState(false)
  const [preview, setPreview] = useState<EventPreview | null>(null)
  const [previewKey, setPreviewKey] = useState('')
  const [previewLoading, setPreviewLoading] = useState(false)
  const [notice, setNotice] = useState<Notice | null>(null)
  const [duplicateAction, setDuplicateAction] = useState<CreateAction | null>(null)
  const [filters, setFilters] = useState<FilterState>(defaultFilters)
  const [recentPage, setRecentPage] = useState(0)
  const latestPreviewKey = useRef('')
  const duplicateDialogRef = useRef<HTMLElement | null>(null)
  const duplicateTriggerRef = useRef<HTMLElement | null>(null)

  const healthQuery = useQuery({
    queryKey: ['health'],
    queryFn: trackerApi.getHealth,
    retry: false,
    refetchOnWindowFocus: false,
  })
  const bootstrapQuery = useQuery({
    queryKey: ['bootstrap'],
    queryFn: trackerApi.getBootstrap,
    retry: false,
    refetchOnWindowFocus: false,
  })
  const pendingQuery = useQuery({
    queryKey: ['events', 'pending'],
    queryFn: () => trackerApi.listEvents({ status: 'pending', limit: 50, offset: 0 }),
    retry: false,
    refetchOnWindowFocus: false,
  })
  const recentQuery = useQuery({
    queryKey: ['events', 'recent', filters, recentPage],
    queryFn: () =>
      trackerApi.listEvents({
        status: filters.status || undefined,
        event_date: filters.eventDate || undefined,
        calendar_name: filters.calendarName || undefined,
        limit: 20,
        offset: recentPage * 20,
      }),
    retry: false,
    refetchOnWindowFocus: false,
  })

  const bootstrapForm = useMemo(
    () => (bootstrapQuery.data ? formFromBootstrap(bootstrapQuery.data) : emptyForm),
    [bootstrapQuery.data],
  )
  const effectiveForm = !formInitialized ? bootstrapForm : form
  const formErrors = useMemo(() => getFormErrors(effectiveForm), [effectiveForm])
  const formInput = useMemo(() => {
    if (Object.keys(formErrors).length > 0) return null
    return formToEventInput(effectiveForm)
  }, [effectiveForm, formErrors])
  const serializedInput = formInput ? JSON.stringify(formInput) : ''
  const currentPreview = previewKey === serializedInput ? preview : null

  const createMutation = useMutation({
    mutationFn: (command: EventInput & { action: CreateAction; allow_duplicate: boolean }) =>
      trackerApi.createEvent(command),
  })
  const syncMutation = useMutation({ mutationFn: (id: string) => trackerApi.syncEvent(id) })
  const syncPendingMutation = useMutation({ mutationFn: trackerApi.syncPending })

  const runPreview = (input: EventInput, key: string) => {
    latestPreviewKey.current = key
    setPreview(null)
    setPreviewKey('')
    setPreviewLoading(true)
    void trackerApi.previewEvent(input).then(
      (result) => {
        if (latestPreviewKey.current === key) {
          setPreview(result)
          setPreviewKey(key)
          setPreviewLoading(false)
        }
      },
      (error: unknown) => {
        if (latestPreviewKey.current === key) {
          setPreview(null)
          setPreviewKey('')
          setPreviewLoading(false)
          setNotice({ tone: 'danger', title: '无法生成预览', message: getErrorMessage(error) })
        }
      },
    )
  }

  useEffect(() => {
    if (!formInput || !serializedInput) return

    const timer = window.setTimeout(() => {
      runPreview(formInput, serializedInput)
    }, 300)
    return () => window.clearTimeout(timer)
  }, [formInput, serializedInput])

  useEffect(() => {
    if (!duplicateAction) return
    const returnFocus = duplicateTriggerRef.current
    const focusTimer = window.setTimeout(() => {
      duplicateDialogRef.current?.querySelector<HTMLButtonElement>('.button-primary')?.focus()
    }, 0)
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        setDuplicateAction(null)
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => {
      window.clearTimeout(focusTimer)
      document.removeEventListener('keydown', handleKeyDown)
      returnFocus?.focus()
      duplicateTriggerRef.current = null
    }
  }, [duplicateAction])

  const refreshQueries = async () => {
    await Promise.all([
      healthQuery.refetch(),
      bootstrapQuery.refetch(),
      pendingQuery.refetch(),
      recentQuery.refetch(),
    ])
  }

  const resetFormForNextEntry = () => {
    if (!bootstrapQuery.data) {
      setForm(emptyForm)
      return
    }
    setForm(formFromBootstrap(bootstrapQuery.data))
  }

  const requestPreview = () => {
    if (!formInput || !serializedInput) {
      setNotice({ tone: 'info', title: '请先完成预览', message: '填写完整后等待后端返回结束时间和数量。' })
      return
    }
    runPreview(formInput, serializedInput)
  }

  const createEvent = (action: CreateAction, allowDuplicate = false) => {
    if (!formInput || !currentPreview || !previewKey || previewKey !== serializedInput) {
      setNotice({ tone: 'info', title: '请先完成预览', message: '填写完整后等待后端返回结束时间和数量。' })
      return
    }
    if (!duplicateAction && document.activeElement instanceof HTMLElement) {
      duplicateTriggerRef.current = document.activeElement
    }
    setDuplicateAction(null)
    createMutation.mutate(
      { ...formInput, action, allow_duplicate: allowDuplicate },
      {
        onSuccess: async (result) => {
          setNotice(outcomeNotice(result))
          resetFormForNextEntry()
          setPreview(null)
          await Promise.all([
            queryClient.invalidateQueries({ queryKey: ['bootstrap'] }),
            queryClient.invalidateQueries({ queryKey: ['events'] }),
          ])
        },
        onError: async (error) => {
          if (error instanceof TrackerApiError && error.code === 'duplicate_event') {
            setDuplicateAction(action)
            return
          }
          if (error instanceof TrackerApiError && error.timedOut) {
            setNotice({
              tone: 'danger',
              title: '创建请求结果未知',
              message: '没有自动重试。正在重新读取最近记录，请确认后再继续。',
            })
            await Promise.all([
              queryClient.invalidateQueries({ queryKey: ['bootstrap'] }),
              queryClient.invalidateQueries({ queryKey: ['events'] }),
            ])
            return
          }
          setNotice({ tone: 'danger', title: '记录未确认保存', message: getErrorMessage(error) })
        },
      },
    )
  }

  const syncOne = (eventId: string) => {
    syncMutation.mutate(eventId, {
      onSuccess: (result) => {
        setNotice(outcomeNotice(result))
        void Promise.all([
          queryClient.invalidateQueries({ queryKey: ['bootstrap'] }),
          queryClient.invalidateQueries({ queryKey: ['events'] }),
        ])
      },
      onError: (error) => {
        setNotice({ tone: 'danger', title: '同步请求未完成', message: getErrorMessage(error) })
      },
    })
  }

  const syncAll = () => {
    syncPendingMutation.mutate(undefined, {
      onSuccess: (result) => {
        const tone = result.failed > 0 ? 'warning' : 'success'
        setNotice({
          tone,
          title: result.failed > 0 ? '批量同步部分完成' : '批量同步完成',
          message: `尝试 ${result.attempted} 条，成功 ${result.succeeded} 条，失败 ${result.failed} 条。`,
        })
        void Promise.all([
          queryClient.invalidateQueries({ queryKey: ['bootstrap'] }),
          queryClient.invalidateQueries({ queryKey: ['events'] }),
        ])
      },
      onError: (error) => {
        setNotice({ tone: 'danger', title: '批量同步请求未完成', message: getErrorMessage(error) })
      },
    })
  }

  const serviceLoading = healthQuery.isLoading || bootstrapQuery.isLoading
  const serviceError = healthQuery.error ?? bootstrapQuery.error
  const calendarSupported = healthQuery.data?.calendar_supported === true
  const pendingItems = pendingQuery.data?.items ?? []
  const recentItems = recentQuery.data?.items ?? []
  const pendingCount = bootstrapQuery.data?.pending_count ?? pendingQuery.data?.total_count ?? 0
  const mutationBusy = createMutation.isPending || syncMutation.isPending || syncPendingMutation.isPending
  const showListLoading = pendingQuery.isLoading || recentQuery.isLoading

  const updateField = (field: keyof FormState, value: string) => {
    setNotice(null)
    setPreviewLoading(false)
    setFormInitialized(true)
    setForm((current) => ({ ...(!formInitialized && bootstrapQuery.data ? formFromBootstrap(bootstrapQuery.data) : current), [field]: value }))
  }

  const applySuggestion = (suggestion: { calendar_name: string; title: string; next_first_item: number }) => {
    setPreviewLoading(false)
    setFormInitialized(true)
    setForm((current) => ({
      ...(!formInitialized && bootstrapQuery.data ? formFromBootstrap(bootstrapQuery.data) : current),
      calendar_name: suggestion.calendar_name,
      title: suggestion.title,
      first_item: String(suggestion.next_first_item),
      last_item: String(suggestion.next_first_item + 1),
    }))
  }

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">
            <CalendarClock size={18} strokeWidth={2.2} />
          </div>
          <div>
            <p className="brand-name">Time Tracker</p>
            <p className="brand-subtitle">Focused sessions, kept visible.</p>
          </div>
        </div>
        <div className="topbar-actions">
          <div className="system-signals" aria-live="polite">
            <span className={`system-signal ${healthQuery.error ? 'is-error' : healthQuery.data ? 'is-good' : 'is-loading'}`}>
              <Server size={14} aria-hidden="true" />
              {healthQuery.error ? 'Backend 未连接' : healthQuery.data ? 'Backend 正常' : 'Backend 检查中'}
            </span>
            <span className={`system-signal ${healthQuery.data?.calendar_supported === false ? 'is-warning' : healthQuery.data ? 'is-good' : 'is-loading'}`}>
              <CircleDot size={14} aria-hidden="true" />
              {healthQuery.data?.calendar_supported === false ? 'Calendar 不支持' : healthQuery.data ? 'Calendar 平台支持' : 'Calendar 检查中'}
            </span>
          </div>
          <button
            className="icon-button"
            type="button"
            title="刷新服务状态和记录"
            aria-label="刷新服务状态和记录"
            onClick={() => void refreshQueries()}
            disabled={serviceLoading || mutationBusy}
          >
            <RefreshCw size={17} className={serviceLoading ? 'spin' : undefined} aria-hidden="true" />
          </button>
        </div>
      </header>

      {serviceError ? (
        <section className="connection-banner" role="alert">
          <div className="banner-icon"><WifiOff size={18} aria-hidden="true" /></div>
          <div>
            <strong>无法连接本地 backend</strong>
            <p>{getErrorMessage(serviceError, '请先启动 tracker backend，再重试。')}</p>
            <code>cargo run -p tracker-backend --locked -- --database ./data/tracker.sqlite3</code>
          </div>
          <button className="button button-secondary banner-action" type="button" onClick={() => void refreshQueries()}>
            <RotateCw size={15} aria-hidden="true" /> 重试
          </button>
        </section>
      ) : null}

      <main className="workspace">
        <section className="entry-column" aria-labelledby="entry-heading">
          <div className="section-heading">
            <div>
              <p className="section-kicker">NEW SESSION</p>
              <h1 id="entry-heading">记录一段专注时间</h1>
              <p className="section-description">把今天完成的范围留下一条清晰记录。</p>
            </div>
            <div className="time-stamp"><Clock3 size={14} aria-hidden="true" /> {formatDateLabel(effectiveForm.event_date) || '—'}</div>
          </div>

          <form className="entry-form" onSubmit={(event) => { event.preventDefault(); requestPreview() }}>
            <div className="form-grid">
              <label className="field field-wide">
                <span>日历</span>
                <input
                  list="calendar-options"
                  value={effectiveForm.calendar_name}
                  onChange={(event) => updateField('calendar_name', event.target.value)}
                  placeholder="选择或输入日历"
                  aria-invalid={Boolean(formErrors.calendar_name)}
                  aria-describedby={formErrors.calendar_name ? 'calendar-error' : undefined}
                />
                {formErrors.calendar_name ? <small id="calendar-error" className="field-error">{formErrors.calendar_name}</small> : null}
              </label>
              <label className="field">
                <span>日期</span>
                <input type="date" value={effectiveForm.event_date} readOnly aria-describedby="date-help" />
                <small id="date-help" className="field-help">默认使用今天</small>
              </label>
              <label className="field">
                <span>开始时间</span>
                <input
                  type="time"
                  step={600}
                  value={effectiveForm.start_time}
                  onChange={(event) => updateField('start_time', event.target.value)}
                  aria-invalid={Boolean(formErrors.start_time)}
                  aria-describedby={formErrors.start_time ? 'start-time-error' : undefined}
                />
                {formErrors.start_time ? <small id="start-time-error" className="field-error">{formErrors.start_time}</small> : null}
              </label>
              <label className="field field-wide">
                <span>事件</span>
                <input
                  list="title-options"
                  value={effectiveForm.title}
                  onChange={(event) => updateField('title', event.target.value)}
                  placeholder="例如：资料分析"
                  aria-invalid={Boolean(formErrors.title)}
                  aria-describedby={formErrors.title ? 'title-error' : undefined}
                />
                {formErrors.title ? <small id="title-error" className="field-error">{formErrors.title}</small> : null}
              </label>
              <label className="field">
                <span>起始编号</span>
                <input
                  type="number"
                  inputMode="numeric"
                  value={effectiveForm.first_item}
                  onChange={(event) => updateField('first_item', event.target.value)}
                  placeholder="205"
                  aria-invalid={Boolean(formErrors.first_item)}
                  aria-describedby={formErrors.first_item ? 'first-item-error' : undefined}
                />
                {formErrors.first_item ? <small id="first-item-error" className="field-error">{formErrors.first_item}</small> : null}
              </label>
              <label className="field">
                <span>结束编号</span>
                <input
                  type="number"
                  inputMode="numeric"
                  value={effectiveForm.last_item}
                  onChange={(event) => updateField('last_item', event.target.value)}
                  placeholder="206"
                  aria-invalid={Boolean(formErrors.last_item)}
                  aria-describedby={formErrors.last_item ? 'last-item-error' : undefined}
                />
                {formErrors.last_item ? <small id="last-item-error" className="field-error">{formErrors.last_item}</small> : null}
              </label>
            </div>

            <div className={`preview-panel ${currentPreview ? 'is-ready' : ''}`} aria-live="polite">
              <div className="preview-heading">
                <div className="preview-title"><Sparkles size={15} aria-hidden="true" /> 后端预览</div>
                {previewLoading ? <span className="preview-status">计算中…</span> : null}
              </div>
              {currentPreview ? (
                <div className="preview-content">
                  <div className="preview-primary">
                    <strong>{currentPreview.start_time.slice(0, 5)} <span>到</span> {currentPreview.end_time.slice(0, 5)}</strong>
                    <span>{currentPreview.title}</span>
                  </div>
                  <div className="preview-metrics">
                    <div><span>范围</span><strong>{currentPreview.first_item}-{currentPreview.last_item}</strong></div>
                    <div><span>数量</span><strong>{currentPreview.quantity}</strong></div>
                  </div>
                  {currentPreview.duplicate_status ? (
                    <div className="duplicate-hint"><AlertCircle size={15} aria-hidden="true" /> 已有相同内容的 {currentPreview.duplicate_status === 'synced' ? '已同步' : '待同步'} 记录，提交时需要确认。</div>
                  ) : null}
                </div>
              ) : (
                <p className="preview-placeholder">填写完整后，这里会显示 backend 返回的结束时间和数量。</p>
              )}
            </div>

            <div className="form-actions">
              <button className="button button-ghost" type="button" onClick={requestPreview} disabled={!formInput || previewLoading || mutationBusy}>
                <Sparkles size={15} aria-hidden="true" /> 预览
              </button>
              <div className="primary-actions">
              <button className="button button-secondary" type="button" onClick={() => createEvent('save')} disabled={!currentPreview || previewKey !== serializedInput || mutationBusy || serviceLoading}>
                  <Database size={15} aria-hidden="true" /> 仅保存
                </button>
                <button className="button button-primary" type="button" onClick={() => createEvent('save_and_sync')} disabled={!currentPreview || previewKey !== serializedInput || mutationBusy || serviceLoading || !calendarSupported}>
                  <Check size={15} aria-hidden="true" /> 保存并同步
                </button>
              </div>
            </div>
            {!calendarSupported && healthQuery.data ? <p className="action-help"><AlertCircle size={14} aria-hidden="true" /> 当前环境不支持 Calendar，可使用“仅保存”。</p> : null}
          </form>

          {notice ? (
            <div className={`notice notice-${notice.tone}`} role={notice.tone === 'danger' ? 'alert' : 'status'}>
              <div className="notice-icon">
                {notice.tone === 'success' ? <CheckCircle2 size={17} aria-hidden="true" /> : <AlertCircle size={17} aria-hidden="true" />}
              </div>
              <div><strong>{notice.title}</strong><p>{notice.message}</p></div>
              <button className="notice-close" type="button" aria-label="关闭提示" title="关闭提示" onClick={() => setNotice(null)}><X size={15} aria-hidden="true" /></button>
            </div>
          ) : null}
        </section>

        <aside className="activity-column" aria-labelledby="activity-heading">
          <div className="activity-heading">
            <div>
              <p className="section-kicker">ACTIVITY</p>
              <h2 id="activity-heading">记录动态</h2>
            </div>
            <span className="pending-count"><span>{pendingCount}</span> 待同步</span>
          </div>

          <section className="queue-section" aria-labelledby="pending-heading">
            <div className="subsection-heading">
              <div><h3 id="pending-heading">待同步</h3><span>{pendingItems.length ? `${pendingItems.length} 条需要处理` : '保持队列清爽'}</span></div>
              <button className="button button-small" type="button" onClick={syncAll} disabled={pendingItems.length === 0 || mutationBusy || !calendarSupported}>
                <RotateCw size={14} aria-hidden="true" /> 同步全部
              </button>
            </div>
            {showListLoading && pendingQuery.isLoading ? <ListSkeleton /> : pendingItems.length === 0 ? <EmptyState icon={<CheckCircle2 size={22} />} title="暂无待同步记录" message="新记录会出现在这里。" /> : (
              <div className="event-list">
                {pendingItems.map((event) => <EventRow key={event.id} event={event} onSync={syncOne} busy={mutationBusy} />)}
              </div>
            )}
          </section>

          <section className="queue-section recent-section" aria-labelledby="recent-heading">
            <div className="subsection-heading subsection-heading-with-filters">
              <div><h3 id="recent-heading">最近记录</h3><span>{recentQuery.data ? `${recentQuery.data.total_count} 条记录` : '按时间倒序'}</span></div>
              <ListFilter size={16} aria-hidden="true" />
            </div>
            <div className="filters" aria-label="记录筛选">
              <select value={filters.status} onChange={(event) => { setRecentPage(0); setFilters((current) => ({ ...current, status: event.target.value as FilterState['status'] })) }} aria-label="按状态筛选">
                <option value="">全部状态</option>
                <option value="synced">已同步</option>
                <option value="pending">待同步</option>
              </select>
              <input type="date" value={filters.eventDate} onChange={(event) => { setRecentPage(0); setFilters((current) => ({ ...current, eventDate: event.target.value })) }} aria-label="按日期筛选" />
              <input list="calendar-options" value={filters.calendarName} onChange={(event) => { setRecentPage(0); setFilters((current) => ({ ...current, calendarName: event.target.value })) }} placeholder="日历" aria-label="按日历筛选" />
            </div>
            {recentQuery.isLoading ? <ListSkeleton /> : recentQuery.error ? <InlineError error={recentQuery.error} onRetry={() => void recentQuery.refetch()} /> : recentItems.length === 0 ? <EmptyState icon={<Clock3 size={22} />} title="还没有匹配记录" message="调整筛选条件或先记录一段时间。" /> : (
              <>
                <div className="event-list">
                  {recentItems.map((event) => <EventRow key={event.id} event={event} onSync={syncOne} busy={mutationBusy} />)}
                </div>
                <div className="pagination">
                  <span>第 {recentPage + 1} 页</span>
                  <div>
                    <button className="icon-button icon-button-small" type="button" aria-label="上一页" title="上一页" onClick={() => setRecentPage((page) => Math.max(0, page - 1))} disabled={recentPage === 0 || recentQuery.isFetching}><ChevronLeft size={15} aria-hidden="true" /></button>
                    <button className="icon-button icon-button-small" type="button" aria-label="下一页" title="下一页" onClick={() => setRecentPage((page) => page + 1)} disabled={(recentPage + 1) * 20 >= (recentQuery.data?.total_count ?? 0) || recentQuery.isFetching}><ChevronRight size={15} aria-hidden="true" /></button>
                  </div>
                </div>
              </>
            )}
          </section>
        </aside>
      </main>

      <datalist id="calendar-options">{bootstrapQuery.data?.calendar_names.map((name) => <option key={name} value={name} />)}</datalist>
      <datalist id="title-options">{bootstrapQuery.data?.titles.map((title) => <option key={title} value={title} />)}</datalist>

      {bootstrapQuery.data?.recent_suggestions.length ? (
        <section className="suggestion-rail" aria-label="历史建议">
          <div className="suggestion-label"><Sparkles size={14} aria-hidden="true" /> 快速接续</div>
          <div className="suggestion-list">
            {bootstrapQuery.data.recent_suggestions.slice(0, 4).map((suggestion) => (
              <button key={`${suggestion.calendar_name}-${suggestion.title}`} type="button" className="suggestion-item" onClick={() => applySuggestion(suggestion)}>
                <span>{suggestion.title}</span><small>{suggestion.calendar_name} · {suggestion.next_first_item}</small>
              </button>
            ))}
          </div>
        </section>
      ) : null}

      {duplicateAction ? (
        <div className="dialog-backdrop" role="presentation">
          <section ref={duplicateDialogRef} className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="duplicate-title">
            <div className="dialog-icon"><AlertCircle size={20} aria-hidden="true" /></div>
            <div className="dialog-copy">
              <h2 id="duplicate-title">这条记录已经存在</h2>
              <p>当前内容与历史记录相同。仍然保存会创建一条新的记录。</p>
              <div className="dialog-summary"><strong>{effectiveForm.title || '未命名事件'}</strong><span>{effectiveForm.event_date} · {effectiveForm.start_time} · {effectiveForm.first_item}-{effectiveForm.last_item}</span></div>
              <div className="dialog-actions">
                <button className="button button-ghost" type="button" onClick={() => setDuplicateAction(null)}>取消</button>
                <button className="button button-primary" type="button" onClick={() => createEvent(duplicateAction, true)} disabled={createMutation.isPending}>仍然保存</button>
              </div>
            </div>
          </section>
        </div>
      ) : null}
    </div>
  )
}

function EventRow({ event, onSync, busy }: { event: EventView; onSync: (id: string) => void; busy: boolean }) {
  const tone = statusTone(event)
  return (
    <article className={`event-row event-row-${tone}`}>
      <div className="event-row-main">
        <div className="event-row-title"><span className="status-dot" aria-hidden="true" /> <strong>{event.title}</strong></div>
        <span className="event-row-time">{event.start_time.slice(0, 5)}-{event.end_time.slice(0, 5)}</span>
      </div>
      <div className="event-row-meta"><span>{event.calendar_name}</span><span>{formatDateLabel(event.event_date)}</span><span>{event.first_item}-{event.last_item} · {event.quantity}</span></div>
      <div className="event-row-footer">
        <span className="event-status"><span>{statusLabel(event)}</span>{event.sync_attempts > 0 ? <small>尝试 {event.sync_attempts} 次</small> : null}</span>
        {event.sync_status === 'pending' ? <button className="row-action" type="button" onClick={() => onSync(event.id)} disabled={busy} title="重试同步"><RotateCw size={14} aria-hidden="true" /> 重试</button> : <span className="synced-mark"><Check size={14} aria-hidden="true" /> 完成</span>}
      </div>
      {event.last_sync_error ? <p className="event-error"><AlertCircle size={13} aria-hidden="true" /> {event.last_sync_error.message}</p> : null}
    </article>
  )
}

function ListSkeleton() {
  return <div className="list-skeleton" aria-label="加载中"><span /><span /><span /></div>
}

function EmptyState({ icon, title, message }: { icon: React.ReactNode; title: string; message: string }) {
  return <div className="empty-state"><div className="empty-icon">{icon}</div><strong>{title}</strong><span>{message}</span></div>
}

function InlineError({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  return <div className="inline-error" role="alert"><AlertCircle size={16} aria-hidden="true" /><span>{getErrorMessage(error)}</span><button type="button" onClick={onRetry}>重试</button></div>
}

export default App
