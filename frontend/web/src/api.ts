import { z } from 'zod'

const SyncStatusSchema = z.enum(['pending', 'synced'])
const CalendarActionSchema = z.enum(['created', 'existing'])
const SyncOutcomeStatusSchema = z.enum([
  'not_requested',
  'succeeded',
  'failed',
  'already_synced',
])

const OperationErrorSchema = z.object({
  code: z.string(),
  message: z.string(),
})

export const EventInputSchema = z.object({
  calendar_name: z.string(),
  event_date: z.string(),
  start_time: z.string(),
  end_time: z.string(),
  title: z.string(),
  first_item: z.number().int(),
  last_item: z.number().int(),
})

export const BootstrapViewSchema = z.object({
  server_date: z.string(),
  calendar_names: z.array(z.string()),
  titles: z.array(z.string()),
  recent_suggestions: z.array(
    z.object({
      calendar_name: z.string(),
      title: z.string(),
      next_first_item: z.number().int(),
    }),
  ),
  pending_count: z.number().int(),
})

export const HealthViewSchema = z.object({
  status: z.literal('ok'),
  database: z.literal('ok'),
  calendar_supported: z.boolean(),
})

export const EventPreviewSchema = z.object({
  calendar_name: z.string(),
  event_date: z.string(),
  start_time: z.string(),
  end_time: z.string(),
  title: z.string(),
  first_item: z.number().int(),
  last_item: z.number().int(),
  quantity: z.number().int(),
  duplicate_status: SyncStatusSchema.nullable().optional(),
})

export const EventViewSchema = z.object({
  id: z.string(),
  calendar_name: z.string(),
  event_date: z.string(),
  start_time: z.string(),
  end_time: z.string(),
  title: z.string(),
  first_item: z.number().int(),
  last_item: z.number().int(),
  quantity: z.number().int(),
  sync_status: SyncStatusSchema,
  sync_attempts: z.number().int(),
  last_sync_error: OperationErrorSchema.nullable().optional(),
})

export const SyncOutcomeSchema = z.object({
  status: SyncOutcomeStatusSchema,
  action: CalendarActionSchema.nullable().optional(),
  error: OperationErrorSchema.nullable().optional(),
})

export const CreateEventResultSchema = z.object({
  event: EventViewSchema,
  save_status: z.string(),
  sync: SyncOutcomeSchema,
})

export const SyncEventResultSchema = z.object({
  event: EventViewSchema,
  sync: SyncOutcomeSchema,
})

export const BatchSyncResultSchema = z.object({
  attempted: z.number().int(),
  succeeded: z.number().int(),
  failed: z.number().int(),
  results: z.array(SyncEventResultSchema),
})

export const EventPageSchema = z.object({
  items: z.array(EventViewSchema),
  total_count: z.number().int(),
  limit: z.number().int(),
  offset: z.number().int(),
})

const ErrorEnvelopeSchema = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
    fields: z.record(z.string(), z.string()).optional().default({}),
  }),
})

export type EventInput = z.infer<typeof EventInputSchema>
export type BootstrapView = z.infer<typeof BootstrapViewSchema>
export type HealthView = z.infer<typeof HealthViewSchema>
export type EventPreview = z.infer<typeof EventPreviewSchema>
export type EventView = z.infer<typeof EventViewSchema>
export type SyncOutcome = z.infer<typeof SyncOutcomeSchema>
export type CreateEventResult = z.infer<typeof CreateEventResultSchema>
export type SyncEventResult = z.infer<typeof SyncEventResultSchema>
export type BatchSyncResult = z.infer<typeof BatchSyncResultSchema>
export type EventPage = z.infer<typeof EventPageSchema>

export type EventFilter = {
  status?: 'pending' | 'synced'
  event_date?: string
  calendar_name?: string
  limit?: number
  offset?: number
}

export type CreateAction = 'save' | 'save_and_sync'

export class TrackerApiError extends Error {
  readonly kind: 'transport_error' | 'protocol_error' | 'business_error'
  readonly code?: string
  readonly fields: Record<string, string>
  readonly status?: number
  readonly timedOut: boolean

  constructor(
    message: string,
    options: {
      kind: TrackerApiError['kind']
      code?: string
      fields?: Record<string, string>
      status?: number
      timedOut?: boolean
    },
  ) {
    super(message)
    this.name = 'TrackerApiError'
    this.kind = options.kind
    this.code = options.code
    this.fields = options.fields ?? {}
    this.status = options.status
    this.timedOut = options.timedOut ?? false
  }
}

const baseUrl = (import.meta.env.VITE_API_BASE_URL ?? '').replace(/\/$/, '')

async function requestJson<T>(
  path: string,
  schema: z.ZodType<T>,
  init?: RequestInit,
): Promise<T> {
  const controller = new AbortController()
  const timeoutMs = init?.method === 'POST' ? 75_000 : 15_000
  const timeout = window.setTimeout(() => controller.abort(), timeoutMs)

  try {
    const response = await fetch(`${baseUrl}${path}`, {
      ...init,
      headers: {
        Accept: 'application/json',
        ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
        ...init?.headers,
      },
      signal: controller.signal,
    })
    const raw = await response.text()
    let payload: unknown
    try {
      payload = raw ? JSON.parse(raw) : null
    } catch {
      throw new TrackerApiError('后端返回了无法解析的 JSON。', {
        kind: 'protocol_error',
        status: response.status,
      })
    }

    if (!response.ok) {
      const parsedError = ErrorEnvelopeSchema.safeParse(payload)
      if (!parsedError.success) {
        throw new TrackerApiError(`后端返回 HTTP ${response.status}，但错误格式不符合约定。`, {
          kind: 'protocol_error',
          status: response.status,
        })
      }
      throw new TrackerApiError(parsedError.data.error.message, {
        kind: 'business_error',
        code: parsedError.data.error.code,
        fields: parsedError.data.error.fields,
        status: response.status,
      })
    }

    const parsed = schema.safeParse(payload)
    if (!parsed.success) {
      throw new TrackerApiError('后端响应字段与 API v1 不一致。', {
        kind: 'protocol_error',
        status: response.status,
      })
    }
    return parsed.data
  } catch (error) {
    if (error instanceof TrackerApiError) {
      throw error
    }
    if (error instanceof DOMException && error.name === 'AbortError') {
      throw new TrackerApiError('等待后端响应超时，结果可能仍在处理中。', {
        kind: 'transport_error',
        timedOut: true,
      })
    }
    throw new TrackerApiError('无法连接 tracker backend。', {
      kind: 'transport_error',
    })
  } finally {
    window.clearTimeout(timeout)
  }
}

export const trackerApi = {
  getHealth: () => requestJson('/health', HealthViewSchema),
  getBootstrap: () => requestJson('/api/v1/bootstrap', BootstrapViewSchema),
  previewEvent: (input: EventInput) =>
    requestJson('/api/v1/events/preview', EventPreviewSchema, {
      method: 'POST',
      body: JSON.stringify(input),
    }),
  createEvent: (command: EventInput & { action: CreateAction; allow_duplicate: boolean }) =>
    requestJson('/api/v1/events', CreateEventResultSchema, {
      method: 'POST',
      body: JSON.stringify(command),
    }),
  listEvents: (filter: EventFilter) => {
    const params = new URLSearchParams()
    if (filter.status) params.set('status', filter.status)
    if (filter.event_date) params.set('event_date', filter.event_date)
    if (filter.calendar_name) params.set('calendar_name', filter.calendar_name)
    params.set('limit', String(filter.limit ?? 50))
    params.set('offset', String(filter.offset ?? 0))
    return requestJson(`/api/v1/events?${params.toString()}`, EventPageSchema)
  },
  syncEvent: (id: string) =>
    requestJson(`/api/v1/events/${encodeURIComponent(id)}/sync`, SyncEventResultSchema, {
      method: 'POST',
    }),
  syncPending: () =>
    requestJson('/api/v1/sync', BatchSyncResultSchema, {
      method: 'POST',
    }),
}

export function formToEventInput(form: FormState): EventInput {
  const endTime = addMinutes(form.start_time, 30)
  return {
    calendar_name: form.calendar_name.trim(),
    event_date: form.event_date,
    start_time: form.start_time,
    end_time: endTime,
    title: form.title.trim(),
    first_item: Number(form.first_item),
    last_item: Number(form.last_item),
  }
}

export type FormState = {
  calendar_name: string
  event_date: string
  start_time: string
  title: string
  first_item: string
  last_item: string
}

export function formFromBootstrap(bootstrap: BootstrapView): FormState {
  const suggestion = bootstrap.recent_suggestions[0]
  const firstItem = suggestion?.next_first_item
  const now = new Date()
  const start = new Date(now)
  start.setSeconds(0, 0)
  start.setMinutes(start.getMinutes() - 30)
  if (start.toDateString() !== now.toDateString()) {
    start.setTime(now.getTime())
    start.setHours(0, 0, 0, 0)
  }
  start.setMinutes(Math.floor(start.getMinutes() / 10) * 10)
  return {
    calendar_name: bootstrap.calendar_names[0] ?? '',
    event_date: bootstrap.server_date,
    start_time: `${String(start.getHours()).padStart(2, '0')}:${String(start.getMinutes()).padStart(2, '0')}`,
    title: bootstrap.titles[0] ?? '',
    first_item: firstItem === undefined ? '' : String(firstItem),
    last_item: firstItem === undefined ? '' : String(firstItem + 1),
  }
}

function addMinutes(value: string, minutes: number): string {
  const [hours, mins] = value.split(':').map(Number)
  const total = hours * 60 + mins + minutes
  const normalized = ((total % 1440) + 1440) % 1440
  return `${String(Math.floor(normalized / 60)).padStart(2, '0')}:${String(normalized % 60).padStart(2, '0')}`
}

export function getFormErrors(form: FormState): Record<string, string> {
  const errors: Record<string, string> = {}
  if (!form.calendar_name.trim()) errors.calendar_name = '请输入日历名称。'
  if (!form.event_date) errors.event_date = '缺少 backend 日期。'
  if (!form.start_time) errors.start_time = '请输入开始时间。'
  else {
    const [hours, minutes] = form.start_time.split(':').map(Number)
    if (!Number.isInteger(hours) || !Number.isInteger(minutes) || hours < 0 || hours > 23 || minutes < 0 || minutes > 59) {
      errors.start_time = '请输入有效的开始时间。'
    } else if (hours * 60 + minutes + 30 >= 1440) {
      errors.start_time = '开始时间需要预留 30 分钟，不能跨越当天。'
    }
  }
  if (!form.title.trim()) errors.title = '请输入事件标题。'
  if (!form.first_item.trim() || !Number.isInteger(Number(form.first_item))) {
    errors.first_item = '请输入整数。'
  }
  if (!form.last_item.trim() || !Number.isInteger(Number(form.last_item))) {
    errors.last_item = '请输入整数。'
  } else if (
    form.first_item.trim() &&
    Number.isInteger(Number(form.first_item)) &&
    Number(form.last_item) < Number(form.first_item)
  ) {
    errors.last_item = '结束编号不能小于起始编号。'
  }
  return errors
}
