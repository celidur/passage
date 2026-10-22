import type {ReactNode} from 'react';
import {ArrowUpRight, Network, Radio, ShieldCheck} from 'lucide-react';
import type {Route, Status} from '../../api/types';
import {pad} from '../../lib/format';
import {isProtected} from './host';

export function StatsGrid({status, routes}: {status: Status; routes: Route[]}) {
  const protectedCount = routes.filter((r) => r.enabled && isProtected(r)).length;
  return (
    <div className="stats">
      <Stat label="Active hosts" icon={<Network size={16} />} value={pad(status.routes)}>
        Routes currently accepting traffic
      </Stat>
      <Stat label="Requests" icon={<Radio size={16} />} value={status.requests.toLocaleString()}>
        Since the gateway started
      </Stat>
      <Stat label="Protected hosts" icon={<ShieldCheck size={16} />} value={pad(protectedCount)}>
        Forward authentication enabled
      </Stat>
      <Stat label="Proxy failures" icon={<ArrowUpRight size={16} />} value={status.failures.toLocaleString()}>
        Errors and responses with status 5xx
      </Stat>
    </div>
  );
}

type StatProps = {label: string; icon: ReactNode; value: string; children: ReactNode};

function Stat({label, icon, value, children}: StatProps) {
  return (
    <article>
      <span>
        {label}
        {icon}
      </span>
      <strong>{value}</strong>
      <small>{children}</small>
    </article>
  );
}
