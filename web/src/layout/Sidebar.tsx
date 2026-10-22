import {Lock, LogOut, Network, Server, ShieldCheck} from 'lucide-react';
import {PAGE_TITLES, type Page} from './pages';

type Props = {
  page: Page;
  onNavigate: (page: Page) => void;
  version?: string;
  hostCount: number;
  certificateCount: number;
  onLock: () => void;
};

export function Sidebar({page, onNavigate, version, hostCount, certificateCount, onLock}: Props) {
  const items = [
    {page: 'hosts' as const, icon: <Network size={18} />, count: hostCount},
    {page: 'certificates' as const, icon: <Lock size={18} />, count: certificateCount},
  ];
  return (
    <aside>
      <a className="brand" href="/">
        <span className="brand-icon">p</span>passage
        {version && <span className="version">{version}</span>}
      </a>
      <div className="workspace">
        <span className="workspace-icon">
          <Server size={19} />
        </span>
        <div>
          Bare-metal gateway<small>Single server</small>
        </div>
        <span className="dot" />
      </div>
      <nav>
        <div className="nav-label">WORKSPACE</div>
        {items.map((item) => (
          <button
            key={item.page}
            className={item.page === page ? 'nav-item active' : 'nav-item'}
            aria-current={item.page === page ? 'page' : undefined}
            onClick={() => onNavigate(item.page)}
          >
            {item.icon}
            {PAGE_TITLES[item.page]}
            <span>{item.count}</span>
          </button>
        ))}
      </nav>
      <div className="sidebar-note">
        <ShieldCheck size={19} />
        <strong>
          Your infrastructure.
          <br />
          Your front door.
        </strong>
        <p>Routing, access, and visibility in one place.</p>
      </div>
      <div className="engine">
        <span className="dot" />
        <div>
          Pingora engine<small>Connected to management API</small>
        </div>
      </div>
      <button className="logout" onClick={onLock}>
        <LogOut size={16} />
        Lock dashboard
      </button>
    </aside>
  );
}
