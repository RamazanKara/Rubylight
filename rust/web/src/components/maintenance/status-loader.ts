/**
 * A panel's status loads. A poll tick is skipped while a load is in flight
 * (the host can take longer than a tick to answer), and only the newest
 * load's answer is applied, so a slow one never overwrites it.
 */
export function statusLoader<T>(fetch: () => Promise<T>, apply: (value: T) => void, fail: (error: unknown) => void) {
  let inFlight = 0;
  let newest = 0;
  async function load() {
    const ticket = ++newest;
    inFlight++;
    try {
      const value = await fetch();
      if (ticket === newest) apply(value);
    } catch (error) {
      if (ticket === newest) fail(error);
    } finally {
      inFlight--;
    }
  }
  /** A poll tick: nothing while a load is in flight. */
  function tick() {
    return inFlight > 0 ? Promise.resolve() : load();
  }
  return { load, tick };
}
