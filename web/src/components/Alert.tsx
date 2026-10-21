import type {ReactNode} from 'react';
import {Check} from 'lucide-react';

export function Notice({children}: {children: ReactNode}) {
  return (
    <div className="notice" role="status">
      <Check size={16} />
      {children}
    </div>
  );
}

export function ErrorMessage({children}: {children: ReactNode}) {
  if (!children) return null;
  return (
    <div className="error" role="alert">
      {children}
    </div>
  );
}
