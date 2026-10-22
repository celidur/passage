import type {ReactNode} from 'react';
import {LockKeyhole} from 'lucide-react';
import {Sidebar} from './Sidebar';
import {PAGE_TITLES, type Page} from './pages';
import './layout.css';

type Props = {
  page: Page;
  onNavigate: (page: Page) => void;
  version?: string;
  hostCount: number;
  certificateCount: number;
  onLock: () => void;
  children: ReactNode;
};

/** Authenticated page frame: sidebar, breadcrumb bar and content area. */
export function AppShell({page, children, ...sidebar}: Props) {
  return (
    <div className="shell">
      <Sidebar page={page} {...sidebar} />
      <main>
        <header>
          <span>
            Workspace <span className="slash">/</span> <b>{PAGE_TITLES[page]}</b>
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
