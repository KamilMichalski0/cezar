import { describe, expect, it } from 'vitest';
import { createServer } from 'node:net';
import { apiOrigin, canListen, pickPort } from './api-origin.ts';

describe('cockpit API origin', () => {
  it('uses the configured bind host and brackets IPv6 literals', () => {
    expect(apiOrigin(undefined, 4321)).toBe('http://127.0.0.1:4321');
    expect(apiOrigin('172.17.0.1', 4321)).toBe('http://172.17.0.1:4321');
    expect(apiOrigin('::1', 4321)).toBe('http://[::1]:4321');
    expect(apiOrigin('[::1]', 4321)).toBe('http://[::1]:4321');
  });

  it('probes and selects ports on the configured interface', async () => {
    const occupied = createServer();
    await new Promise<void>((resolve) => occupied.listen(0, '127.0.0.1', resolve));
    const address = occupied.address();
    if (!address || typeof address === 'string') throw new Error('test server did not get a port');
    const next = await pickPort(address.port, '127.0.0.1');
    expect(next).toBeGreaterThan(address.port);
    await new Promise<void>((resolve, reject) => occupied.close((error) => error ? reject(error) : resolve()));
    expect(await canListen(next, '127.0.0.1')).toBe(true);
  });
});
