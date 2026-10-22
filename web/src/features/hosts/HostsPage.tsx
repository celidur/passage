import {useState} from 'react';
import {Plus, RefreshCw, ShieldCheck} from 'lucide-react';
import type {Route} from '../../api/types';
import {ErrorMessage, Notice} from '../../components/Alert';
import {DeleteHostDialog} from './DeleteHostDialog';
import {EmptyState} from './EmptyState';
import {HostFormDialog} from './HostFormDialog';
import {HostsTable} from './HostsTable';
import {StatsGrid} from './StatsGrid';
import {matchesQuery} from './host';
import type {Hosts} from './useHosts';
import './hosts.css';

type Editing = {route: Route | null} | null;

export function HostsPage({hosts}: {hosts: Hosts}) {
  const {routes, upstreams, status, loadError, refresh, save, remove} = hosts;
  const [query, setQuery] = useState('');
  const [notice, setNotice] = useState('');
  const [editing, setEditing] = useState<Editing>(null);
  const [deleting, setDeleting] = useState<Route | null>(null);

  const visible = routes.filter((route) => matchesQuery(route, query));

  function openEditor(route: Route | null) {
    setNotice('');
    setEditing({route});
  }

  return (
    <>
      <div className="title-row">
        <div>
          <div className="eyebrow">TRAFFIC, UNDER YOUR CONTROL</div>
          <h1>
            Proxy hosts<span className="title-dot">.</span>
          </h1>
          <p>The front door to everything you run.</p>
        </div>
        <button className="primary" onClick={() => openEditor(null)}>
          <Plus size={17} />
          Add proxy host
        </button>
      </div>

      {status && <StatsGrid status={status} routes={routes} />}
      {notice && <Notice>{notice}</Notice>}
      <ErrorMessage>{loadError}</ErrorMessage>

      <section className="hosts">
        <div className="table-top">
          <div>
            <h2>
              Your services <span>{routes.length}</span>
            </h2>
            <p>Changes apply immediately, without a restart.</p>
          </div>
          <div className="tools">
            <input
              aria-label="Search hosts"
              placeholder="Search hosts…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <button className="icon" aria-label="Refresh hosts" onClick={() => void refresh()}>
              <RefreshCw size={16} />
            </button>
          </div>
        </div>
        {visible.length ? (
          <HostsTable routes={visible} upstreams={upstreams} onEdit={openEditor} onDelete={setDeleting} />
        ) : (
          <EmptyState searching={query !== ''} onAdd={() => openEditor(null)} />
        )}
        <div className="table-bottom">
          <span>
            <span className="dot" />
            Live configuration
          </span>
          <span>
            {visible.length} of {routes.length} hosts
          </span>
        </div>
      </section>

      <div className="bottom-note">
        <ShieldCheck size={16} />
        <p>
          SSO integrates through your identity provider’s forward-auth endpoint.
          <br />
          <span>Native OIDC and automatic certificates are planned for a later release.</span>
        </p>
        {status && <span className="release">PASSAGE / v{status.version}</span>}
      </div>

      {editing && (
        <HostFormDialog
          route={editing.route}
          onClose={() => setEditing(null)}
          onSave={async (input) => {
            await save(editing.route?.id ?? null, input);
            setEditing(null);
            setNotice('Route saved. Changes are live.');
          }}
        />
      )}
      {deleting && (
        <DeleteHostDialog
          route={deleting}
          onClose={() => setDeleting(null)}
          onConfirm={async () => {
            await remove(deleting.id);
            setDeleting(null);
            setNotice('Route removed.');
          }}
        />
      )}
    </>
  );
}
