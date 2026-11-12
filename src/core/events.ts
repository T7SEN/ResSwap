import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AppState } from './types/AppState';
import type { CoreEvent } from './types/CoreEvent';
import type { CountdownTick } from './types/CountdownTick';
import type { ModeReverted } from './types/ModeReverted';
import type { PendingChange } from './types/PendingChange';
import type { RaisedError } from './types/RaisedError';
import type { UpdateInfo } from './types/UpdateInfo';
import type { WatcherMatch } from './types/WatcherMatch';

/** Stops one subscription. */
export type { UnlistenFn };

/**
 * Accepts the map only when it holds exactly the names of the Rust CoreEvent enum, so a new or
 * renamed event fails pnpm typecheck until this file follows.
 */
type EveryEvent<T extends Record<CoreEvent, unknown> & Record<Exclude<keyof T, CoreEvent>, never>> =
  T;

/** The payload each core event carries. The core sends every event to every open window. */
export type EventMap = EveryEvent<{
  'state-changed': AppState;
  'mode-pending-confirm': PendingChange;
  'countdown-tick': CountdownTick;
  'mode-reverted': ModeReverted;
  'watcher-matched': WatcherMatch;
  'error-raised': RaisedError;
  'update-available': UpdateInfo;
}>;

/**
 * Subscribes to one core event. The promise settles once the subscription is in place, so a
 * caller that awaits it before get_state loses no change.
 */
export function onCoreEvent<E extends CoreEvent>(
  name: E,
  handler: (payload: EventMap[E]) => void,
): Promise<UnlistenFn> {
  return listen<EventMap[E]>(name, (event) => {
    handler(event.payload);
  });
}
