export type AnnouncerProps = {
  polite: string;
  assertive: string;
};

/**
 * The window's two live regions. They are rendered empty at load and stay for the window's life,
 * because a region added together with its text is often not read.
 */
export function Announcer({ polite, assertive }: AnnouncerProps) {
  return (
    <>
      <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {polite}
      </div>
      <div className="sr-only" role="alert" aria-live="assertive" aria-atomic="true">
        {assertive}
      </div>
    </>
  );
}
