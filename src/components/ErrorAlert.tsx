import { CircleAlert, TriangleAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { errorMessage, type MessageHints } from '@/core/errors';
import type { IpcError } from '@/core/types/IpcError';
import { cn } from '@/lib/utils';
import { StepList } from './StepList';

export type ErrorAlertProps = {
  error: IpcError;
  hints?: MessageHints;
  /** A warning inside a successful outcome, such as SCALING_NOT_APPLIED. */
  isWarning?: boolean;
  /** Text or actions that sit under the message for this place. */
  children?: ReactNode;
};

/**
 * One IpcError: its message from errors.ts, then the steps when there are any, then what this
 * place adds, then the technical detail in a closed fold. Not a live region: the window
 * announces the message once through its announcer.
 */
export function ErrorAlert({ error, hints, isWarning = false, children }: ErrorAlertProps) {
  const Icon = isWarning ? TriangleAlert : CircleAlert;
  return (
    <div
      className={cn(
        'flex flex-col gap-2 rounded-lg border p-3',
        isWarning ? 'border-warning' : 'border-destructive',
      )}
    >
      <p className="flex gap-2">
        <Icon
          aria-hidden="true"
          className={cn('mt-0.5 size-4 shrink-0', isWarning ? 'text-warning' : 'text-destructive')}
        />
        <span className="break-words">{errorMessage(error, hints)}</span>
      </p>
      {error.steps.length > 0 ? <StepList steps={error.steps} /> : null}
      {children}
      {error.detail === '' ? null : (
        <details className="text-xs">
          <summary className="w-fit cursor-pointer py-1 text-sm">Details</summary>
          <p className="break-all text-muted-foreground">{error.detail}</p>
        </details>
      )}
    </div>
  );
}
