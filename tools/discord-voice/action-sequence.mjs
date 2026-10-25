// One child-process incarnation owns one action counter across all producers.
// Failed/uncertain sends consume their identity; identities are never replayed.
export class ActionSequence {
  #sequence = 0;
  next(producer) {
    if (!['voice', 'playback'].includes(producer)) throw new Error('Invalid action producer');
    if (this.#sequence === Number.MAX_SAFE_INTEGER) throw new Error('Action sequence exhausted');
    const sequence = ++this.#sequence;
    return { id: `${producer}-${sequence}`, sequence };
  }
}
