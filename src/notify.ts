/** The last notification shown, for whom, and whether the window was away when it came. */
export interface Notified {
  id: string;
  at: number;
  away: boolean;
}

const WITHIN = 60_000;

/** macOS doesn't say which notification was clicked; coming back within a minute of one shown
 *  while you were away is taken as following it. The session to go to, if so. */
export function backTo(n: Notified | null, now: number): string | null {
  return n && n.away && now - n.at <= WITHIN ? n.id : null;
}
