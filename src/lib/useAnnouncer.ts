import { useCallback, useState } from 'react';

/** Which live region a text goes to. */
export type Tone = 'polite' | 'assertive';

/** The text of both live regions and the function that sets one of them. */
export type AnnouncerState = {
  polite: string;
  assertive: string;
  announce: (text: string, tone: Tone) => void;
};

/**
 * Holds the text of the two live regions. A new text clears its region first and arrives on the
 * next frame, so the same message said twice is read twice.
 */
export function useAnnouncer(): AnnouncerState {
  const [regions, setRegions] = useState({ polite: '', assertive: '' });
  const announce = useCallback((text: string, tone: Tone) => {
    setRegions((current) => ({ ...current, [tone]: '' }));
    requestAnimationFrame(() => {
      setRegions((current) => ({ ...current, [tone]: text }));
    });
  }, []);
  return { ...regions, announce };
}
