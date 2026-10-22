import type {ReactNode} from 'react';
import {LockKeyhole} from 'lucide-react';
import {Sidebar} from './Sidebar';
import './layout.css';

type Props = {
  title: string;
  version?: string;
  hostCount: number;
  onLock: () => void;
  children: ReactNode;
};

/** Authenticated page frame: sidebar, breadcrumb bar and content area. */
export function AppShell({title, version, hostCount, onLock, children}: Props) {
  return (
    <div className="shell">
      <Sidebar version={version} hostCount={hostCount} onLock={onLock} />
      <main>
        <header>
          <span>
            Workspace <span className="slash">/</span> <b>{title}</b>
          </span>
          <span className="local">
            <LockKeyhole size={13} />
            Local administration
          </span>
        </header>
        <div className="content">{children}</div>
      </main>
    </div>
  );
}
