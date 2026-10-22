import {useCallback, useMemo, useState} from 'react';
import {createClient, type ApiClient} from './api/client';
import {LoginPage} from './features/auth/LoginPage';
import {HostsPage} from './features/hosts/HostsPage';
import {useHosts} from './features/hosts/useHosts';
import {AppShell} from './layout/AppShell';

/** The token lives only in memory: reloading the page locks the dashboard. */
export function App() {
  const [token, setToken] = useState('');
  const [lockReason, setLockReason] = useState('');

  const lock = useCallback((reason = '') => {
    setToken('');
    setLockReason(reason);
  }, []);
  const client = useMemo(() => (token ? createClient(token, lock) : null), [token, lock]);

  if (!client) return <LoginPage lockReason={lockReason} onUnlock={setToken} />;
  return <Dashboard client={client} onLock={() => lock()} />;
}

function Dashboard({client, onLock}: {client: ApiClient; onLock: () => void}) {
  const hosts = useHosts(client);
  return (
    <AppShell title="Proxy hosts" version={hosts.status?.version} hostCount={hosts.routes.length} onLock={onLock}>
      <HostsPage hosts={hosts} />
    </AppShell>
  );
}
