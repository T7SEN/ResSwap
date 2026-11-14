import type { ReactNode, Ref } from 'react';
import { cn } from '@/lib/utils';

const VARIANTS = {
  primary: 'bg-primary text-primary-foreground hover:bg-primary/90',
  secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary/80',
  outline:
    'border border-input bg-background text-foreground hover:bg-accent hover:text-accent-foreground',
} as const;

export type ActionButtonProps = {
  children: ReactNode;
  onPress: () => void;
  variant?: keyof typeof VARIANTS;
  type?: 'button' | 'submit';
  /**
   * True when the button cannot run now. It stays in the Tab order with aria-disabled, and
   * aria-describedby points at the visible reason, so the player can learn why.
   */
  isBlocked?: boolean;
  /** The id of the visible text that says why the button is blocked. */
  reasonId?: string | undefined;
  /**
   * The accessible name when the visible text sits on two lines. It starts with the first line,
   * so the name a player hears begins with the words they see.
   */
  label?: string;
  className?: string;
  ref?: Ref<HTMLButtonElement>;
};

/** A button that changes something through the core, blocked with a visible reason. */
export function ActionButton({
  children,
  onPress,
  variant = 'outline',
  type = 'button',
  isBlocked = false,
  reasonId,
  label,
  className,
  ref,
}: ActionButtonProps) {
  return (
    <button
      ref={ref}
      type={type}
      aria-label={label}
      aria-disabled={isBlocked ? true : undefined}
      aria-describedby={isBlocked ? reasonId : undefined}
      onClick={(event) => {
        if (isBlocked) {
          // A blocked submit button must not submit its form either.
          event.preventDefault();
          return;
        }
        if (type === 'button') {
          onPress();
        }
      }}
      className={cn(
        'inline-flex min-h-9 items-center justify-center gap-2 rounded-md px-4 py-2 text-left text-sm font-medium',
        isBlocked ? 'cursor-not-allowed bg-muted text-muted-foreground' : VARIANTS[variant],
        className,
      )}
    >
      {children}
    </button>
  );
}
