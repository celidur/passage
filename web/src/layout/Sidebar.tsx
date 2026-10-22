import {LogOut, Network, Server, ShieldCheck} from 'lucide-react';

type Props = {
  version?: string;
  hostCount: number;
  onLock: () => void;
};

export function Sidebar({version, hostCount, onLock}: Props) {
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
        <div className="nav-active">
          <Network size={18} />
          Proxy hosts<span>{hostCount}</span>
        </div>
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
