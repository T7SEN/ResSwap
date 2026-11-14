import { Circle, CircleCheck, CircleX, type LucideIcon, Undo2 } from 'lucide-react';
import type { StepReport } from '@/core/types/StepReport';
import type { StepStatus } from '@/core/types/StepStatus';
import { STEP_LABELS, STEP_STATUS_LABELS } from '@/lib/copy';
import { cn } from '@/lib/utils';

const ICONS: Record<StepStatus, { icon: LucideIcon; colour: string }> = {
  done: { icon: CircleCheck, colour: 'text-success' },
  failed: { icon: CircleX, colour: 'text-destructive' },
  notRun: { icon: Circle, colour: 'text-muted-foreground' },
  undone: { icon: Undo2, colour: 'text-muted-foreground' },
};

export type StepListProps = {
  steps: readonly StepReport[];
};

/**
 * Every step of an operation with its status in words. A step reads as done only when its
 * status is done (REQ-SAFE-17); the icon never carries the meaning alone.
 */
export function StepList({ steps }: StepListProps) {
  return (
    <ol className="flex flex-col gap-1 text-xs">
      {steps.map((report) => {
        const { icon: Icon, colour } = ICONS[report.status];
        return (
          <li key={report.step} className="flex items-center gap-2">
            <Icon aria-hidden="true" className={cn('size-4 shrink-0', colour)} />
            <span>
              {STEP_LABELS[report.step]}: {STEP_STATUS_LABELS[report.status]}
            </span>
          </li>
        );
      })}
    </ol>
  );
}
