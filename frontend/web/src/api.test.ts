import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  formFromBootstrap,
  formToEventInput,
  getFormErrors,
  trackerApi,
  TrackerApiError,
  type BootstrapView,
} from './api'

const bootstrap: BootstrapView = {
  server_date: '2026-09-01',
  calendar_names: ['学习'],
  titles: ['资料分析'],
  recent_suggestions: [
    { calendar_name: '学习', title: '资料分析', next_first_item: 205 },
  ],
  pending_count: 2,
}

beforeEach(() => {
  vi.stubGlobal('window', { setTimeout, clearTimeout })
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('form mapping', () => {
  it('uses backend suggestions and derives the frontend session time', () => {
    const form = formFromBootstrap(bootstrap)
    expect(form).toEqual({
      calendar_name: '学习',
      event_date: '2026-09-01',
      start_time: expect.any(String),
      title: '资料分析',
      first_item: '205',
      last_item: '206',
    })
    expect(Number(form.start_time.slice(3, 5)) % 10).toBe(0)
  })

  it('normalizes text and number input for the API boundary', () => {
    expect(
      formToEventInput({
        calendar_name: ' 学习 ',
        event_date: '2026-09-01',
        start_time: '09:30',
        title: ' 资料分析 ',
        first_item: '205',
        last_item: '218',
      }),
    ).toEqual({
      calendar_name: '学习',
      event_date: '2026-09-01',
      start_time: '09:30',
      end_time: '10:00',
      title: '资料分析',
      first_item: 205,
      last_item: 218,
    })
  })

  it('reports incomplete and inverted ranges locally', () => {
    expect(getFormErrors({
      calendar_name: '',
      event_date: '',
      start_time: '',
      title: '',
      first_item: '8',
      last_item: '7',
    })).toEqual({
      calendar_name: '请输入日历名称。',
      event_date: '缺少 backend 日期。',
      start_time: '请输入开始时间。',
      title: '请输入事件标题。',
      last_item: '结束编号不能小于起始编号。',
    })
  })

  it('keeps the fixed frontend duration within the selected date', () => {
    expect(getFormErrors({
      calendar_name: '学习',
      event_date: '2026-09-01',
      start_time: '23:40',
      title: '资料分析',
      first_item: '1',
      last_item: '2',
    })).toEqual({ start_time: '开始时间需要预留 30 分钟，不能跨越当天。' })
  })
})

describe('trackerApi boundary', () => {
  it('encodes list filters and validates a response', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      expect(String(input)).toBe('/api/v1/events?status=synced&event_date=2026-09-01&calendar_name=%E5%AD%A6%E4%B9%A0&limit=20&offset=40')
      return new Response(JSON.stringify({ items: [], total_count: 0, limit: 20, offset: 40 }), { status: 200 })
    })
    vi.stubGlobal('fetch', fetchMock)

    await expect(trackerApi.listEvents({
      status: 'synced',
      event_date: '2026-09-01',
      calendar_name: '学习',
      limit: 20,
      offset: 40,
    })).resolves.toEqual({ items: [], total_count: 0, limit: 20, offset: 40 })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('keeps backend business errors separate from transport errors', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({
      error: { code: 'duplicate_event', message: '已有相同记录', fields: {} },
    }), { status: 409 })))

    const error = await trackerApi.getBootstrap().catch((value: unknown) => value)
    expect(error).toBeInstanceOf(TrackerApiError)
    expect(error).toMatchObject({ kind: 'business_error', code: 'duplicate_event', status: 409, timedOut: false })
  })

  it('classifies an aborted request as an unknown transport outcome', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => {
      throw new DOMException('aborted', 'AbortError')
    }))

    const error = await trackerApi.syncPending().catch((value: unknown) => value)
    expect(error).toBeInstanceOf(TrackerApiError)
    expect(error).toMatchObject({ kind: 'transport_error', timedOut: true })
  })

  it('rejects successful responses that drift from the contract', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ status: 'ok' }), { status: 200 })))

    const error = await trackerApi.getHealth().catch((value: unknown) => value)
    expect(error).toBeInstanceOf(TrackerApiError)
    expect(error).toMatchObject({ kind: 'protocol_error', status: 200, timedOut: false })
  })
})
