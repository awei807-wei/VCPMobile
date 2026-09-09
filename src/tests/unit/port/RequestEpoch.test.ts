import { describe, expect, it } from 'vitest';
import { RequestEpoch } from '../../../features/logcenter/requestEpoch';

function begin(gate: RequestEpoch) {
  const ticket = gate.begin();
  if (!ticket) throw new Error('expected a ticket');
  return ticket;
}

describe('RequestEpoch', () => {
  it('allows only one request in the same epoch', () => {
    const gate = new RequestEpoch();
    begin(gate);
    expect(gate.begin()).toBeNull();
  });
  it('invalidates results without pretending the request is cancelled', () => {
    const gate = new RequestEpoch();
    const old = begin(gate);
    gate.invalidate();
    expect(gate.isCurrent(old)).toBe(false);
    expect(gate.inFlight).toBe(1);
  });
  it('caps all unfinished invokes at two across epochs', () => {
    const gate = new RequestEpoch();
    const a = begin(gate);
    gate.invalidate();
    begin(gate);
    gate.invalidate();
    expect(gate.begin()).toBeNull();
    gate.finish(a);
    expect(gate.begin()).not.toBeNull();
    expect(gate.inFlight).toBe(2);
  });
  it('obsolete finally does not clear the newer loading owner', () => {
    const gate = new RequestEpoch();
    const a = begin(gate);
    gate.invalidate();
    const b = begin(gate);
    gate.finish(a);
    expect(gate.isCurrent(b)).toBe(true);
    expect(gate.currentInFlight).toBe(1);
    gate.finish(a);
    expect(gate.inFlight).toBe(1);
    gate.finish(b);
    expect(gate.inFlight).toBe(0);
  });
});
