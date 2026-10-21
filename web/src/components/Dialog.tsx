import type {ReactNode} from 'react';
import './dialog.css';

/** Modal container; `titleId` must match the id of the dialog's heading. */
export function Dialog({titleId, children}: {titleId: string; children: ReactNode}) {
  return (
    <div className="overlay">
      <section className="dialog" role="dialog" aria-modal="true" aria-labelledby={titleId}>
        {children}
      </section>
    </div>
  );
}
