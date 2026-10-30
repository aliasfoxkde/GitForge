import { cn } from '@/lib/cn';

interface LoadingSpinnerProps {
  className?: string;
  size?: 'sm' | 'md' | 'lg';
  /** Announced to assistive technology instead of the bare graphic. */
  label?: string;
}

export function LoadingSpinner({ className, size = 'md', label = 'Loading' }: LoadingSpinnerProps) {
  const sizes = {
    sm: 'w-4 h-4',
    md: 'w-8 h-8',
    lg: 'w-12 h-12',
  };

  return (
    <div role="status" aria-live="polite" className={cn('flex items-center justify-center', className)}>
      <span className="sr-only">{label}</span>
      <svg
        aria-hidden="true"
        className={cn('animate-spin motion-reduce:animate-none text-foreground', sizes[size])}
        xmlns="http://www.w3.org/2000/svg"
        fill="none"
        viewBox="0 0 24 24"
      >
        <circle
          className="opacity-25"
          cx="12"
          cy="12"
          r="10"
          stroke="currentColor"
          strokeWidth="4"
        />
        <path
          className="opacity-75"
          fill="currentColor"
          d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
        />
      </svg>
    </div>
  );
}
