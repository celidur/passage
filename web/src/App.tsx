import {useCallback, useMemo, useState} from 'react';
import {createClient, type ApiClient} from './api/client';
import {LoginPage} from './features/auth/LoginPage';
import {CertificatesPage} from './features/certificates/CertificatesPage';
import {useCertificates} from './features/certificates/useCertificates';
import {HostsPage} from './features/hosts/HostsPage';
import {useHosts} from './features/hosts/useHosts';
import {AppShell} from './layout/AppShell';
import type {Page} from './layout/pages';

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
  const [page, setPage] = useState<Page>('hosts');
  const hosts = useHosts(client);
  const certificates = useCertificates(client);
  return (
    <AppShell
      page={page}
      onNavigate={setPage}
      version={hosts.status?.version}
      hostCount={hosts.routes.length}
      certificateCount={certificates.certificates.length}
      onLock={onLock}
    >
      {page === 'hosts' ? (
        <HostsPage hosts={hosts} certificates={certificates} />
      ) : (
        <CertificatesPage certificates={certificates} https={hosts.status?.https} />
      )}
    </AppShell>
  );
}
