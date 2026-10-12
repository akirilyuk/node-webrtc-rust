import { describe, expect, it, vi } from 'vitest'

import { connectAllOrCloseAll, sdpMediaKinds } from './connect-all-or-close.js'

describe('connectAllOrCloseAll', () => {
  it('returns every client in order when all connect', async () => {
    const close = vi.fn()
    await expect(connectAllOrCloseAll([async () => 'a', async () => 'b'], close)).resolves.toEqual([
      'a',
      'b',
    ])
    expect(close).not.toHaveBeenCalled()
  })

  it('closes the clients that connected and rethrows the first rejection', async () => {
    const close = vi.fn()
    await expect(
      connectAllOrCloseAll(
        [
          async () => 'a',
          async () => {
            throw new Error('first')
          },
          async () => 'c',
          async () => {
            throw new Error('second')
          },
        ],
        close,
      ),
    ).rejects.toThrow('first')
    expect(close.mock.calls.map((call) => call[0])).toEqual(['a', 'c'])
  })

  it('waits for the slow attempt before closing, so a late success is not leaked', async () => {
    const close = vi.fn()
    let release!: () => void
    const slow = new Promise<void>((resolve) => {
      release = resolve
    })
    const result = connectAllOrCloseAll(
      [
        async () => {
          throw new Error('fast failure')
        },
        async () => {
          await slow
          return 'late'
        },
      ],
      close,
    )
    release()
    await expect(result).rejects.toThrow('fast failure')
    expect(close).toHaveBeenCalledWith('late')
  })
})

describe('sdpMediaKinds', () => {
  it('lists the m-line kinds', () => {
    expect(sdpMediaKinds('v=0\r\nm=audio 9 UDP\r\na=x\r\nm=application 9 UDP\r\n')).toBe(
      'audio,application',
    )
    expect(sdpMediaKinds(undefined)).toBe('none')
  })
})
