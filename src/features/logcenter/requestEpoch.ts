/** Only local request bookkeeping. invalidate() does NOT cancel an HTTP request. */
export interface RequestTicket {
  readonly epoch: number;
  readonly id: number;
}

export class RequestEpoch {
  private epoch = 0;
  private sequence = 0;
  private readonly active = new Map<number, number>();

  invalidate(): void {
    this.epoch += 1;
  }

  get inFlight(): number {
    return this.active.size;
  }

  get currentInFlight(): number {
    let count = 0;
    for (const epoch of this.active.values()) {
      if (epoch === this.epoch) count += 1;
    }
    return count;
  }

  begin(): RequestTicket | null {
    // One request per epoch; at most two physical invokes including obsolete ones.
    if (this.active.size >= 2 || this.currentInFlight > 0) return null;
    const ticket = Object.freeze({ epoch: this.epoch, id: ++this.sequence });
    this.active.set(ticket.id, ticket.epoch);
    return ticket;
  }

  isCurrent(ticket: RequestTicket): boolean {
    return ticket.epoch === this.epoch && this.active.get(ticket.id) === ticket.epoch;
  }

  finish(ticket: RequestTicket): void {
    // An obsolete finally can release only its own request, never the newer one.
    if (this.active.get(ticket.id) === ticket.epoch) this.active.delete(ticket.id);
  }
}
