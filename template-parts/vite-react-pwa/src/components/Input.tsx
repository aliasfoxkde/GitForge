import { InputHTMLAttributes, forwardRef, useId } from 'react';
import { cn } from '@/lib/cn';

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  helperText?: string;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, label, error, helperText, id, ...props }, ref) => {
    const generatedId = useId();
    const inputId = id || generatedId;
    // Every message is programmatically associated (WCAG 3.3.1/3.3.2) and
    // the error is announced (4.1.3) rather than conveyed by color alone
    // (1.4.1); text tokens clear the AAA 7:1 bar on the dark background.
    const errorId = error ? `${inputId}-error` : undefined;
    const helperId = helperText && !error ? `${inputId}-helper` : undefined;

    return (
      <div className="flex flex-col gap-1.5">
        {label && (
          <label htmlFor={inputId} className="text-sm font-medium">
            {label}
          </label>
        )}
        <input
          ref={ref}
          id={inputId}
          aria-invalid={error ? true : undefined}
          aria-describedby={errorId ?? helperId}
          className={cn(
            'h-11 px-3 rounded-lg border bg-background text-foreground placeholder:text-foreground/50',
            'focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-background',
            'disabled:cursor-not-allowed disabled:opacity-50',
            error ? 'border-danger' : 'border-border',
            className
          )}
          {...props}
        />
        {error && (
          <span id={errorId} role="alert" className="text-sm font-medium text-danger">
            <span aria-hidden="true">⚠ </span>
            {error}
          </span>
        )}
        {helperText && !error && (
          <span id={helperId} className="text-sm text-foreground/80">
            {helperText}
          </span>
        )}
      </div>
    );
  }
);

Input.displayName = 'Input';
