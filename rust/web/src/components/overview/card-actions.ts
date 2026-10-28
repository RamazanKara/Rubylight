import { api, type CoopSession } from '../../lib/api';
import { endConfirmation, joinSteps, removeConfirmation, type Invitee, type Seat } from '../../lib/coop-view';
import { confirm } from '../../lib/feedback.svelte';

/** Runs one card action: marks it busy, says how it went and reloads the card. */
export type Run = (action: string, task: () => Promise<unknown>, done: string, failure: string) => Promise<void>;

/**
 * The co-op card's buttons. Remove and End ask first, and a cancelled question sends nothing.
 * Out of the component so the tests can answer the question (CoopView.test.mjs).
 */
export function cardActions(session: () => CoopSession, run: Run) {
  return {
    async remove(seat: Seat) {
      const current = session();
      if (!(await confirm(removeConfirmation(seat)))) return;
      await run(
        `remove:${seat.slot}`,
        () => api.coop.kick(current.id, seat.slot),
        `${seat.name ?? seat.label} left ${current.title}.`,
        'Removing the player failed',
      );
    },
    async letJoin(device: Invitee) {
      const current = session();
      await run(
        `join:${device.uuid}`,
        () => api.clients.update({ uuid: device.uuid, coop_join: true }),
        `${device.name} can join. ${joinSteps(current)}`,
        `Letting ${device.name} join failed`,
      );
    },
    async end() {
      const current = session();
      const background = current.kind === 'background';
      if (!(await confirm(endConfirmation(current)))) return;
      await run(
        'end',
        () => api.coop.end(current.id),
        background ? `The background stream of ${current.title} is ending.` : `Co-op in ${current.title} is ending.`,
        background ? 'Ending the stream failed' : 'Ending co-op failed',
      );
    },
  };
}
