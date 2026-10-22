import {Lock, Network, Pencil, ShieldCheck, Trash2} from 'lucide-react';
import type {Certificate, Route, RouteUpstreams} from '../../api/types';
import {STATUS_LABEL, certificateFor, daysLeft, tone} from '../certificates/certificate';
import {isProtected} from './host';

type Props = {
  routes: Route[];
  upstreams: Record<string, RouteUpstreams>;
  certificates: Certificate[];
  onEdit: (route: Route) => void;
  onDelete: (route: Route) => void;
};

export function HostsTable({routes, upstreams, certificates, onEdit, onDelete}: Props) {
  return (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>SERVICE / DOMAIN</th>
            <th>UPSTREAM</th>
            <th>ACCESS</th>
            <th>HTTPS</th>
            <th>STATUS</th>
            <th>
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {routes.map((route) => (
            <HostRow
              key={route.id}
              route={route}
              live={upstreams[route.id]}
              certificate={certificateFor(certificates, route.host)}
              onEdit={onEdit}
              onDelete={onDelete}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

type RowProps = {
  route: Route;
  /** Live backends; absent while the route is disabled. */
  live: RouteUpstreams | undefined;
  /** The certificate serving this host, if one exists. */
  certificate: Certificate | undefined;
  onEdit: (route: Route) => void;
  onDelete: (route: Route) => void;
};

function HttpsBadge({route, certificate}: {route: Route; certificate: Certificate | undefined}) {
  if (!route.https.enabled) return <span className="badge">HTTP only</span>;
  if (!certificate) return <span className="badge https bad">No certificate</span>;
  const days = daysLeft(certificate);
  const text = certificate.status === 'valid' && days !== null ? `Valid · ${days}d left` : STATUS_LABEL[certificate.status];
  return (
    <>
      <span className={`badge https ${tone(certificate)}`} title={certificate.last_error ?? undefined}>
        <Lock size={11} />
        {text}
      </span>
      {route.https.redirect && <small className="more">Redirects HTTP</small>}
    </>
  );
}

function UpstreamHealth({live}: {live: RouteUpstreams}) {
  const healthy = live.backends.filter((backend) => backend.healthy).length;
  const total = live.backends.length;
  return (
    <small className={total > 0 && healthy === total ? 'upstream-health' : 'upstream-health degraded'}>
      {total === 0 ? 'No address resolved' : `${healthy} of ${total} addresses healthy`}
    </small>
  );
}

function HostRow({route, live, certificate, onEdit, onDelete}: RowProps) {
  const sso = isProtected(route);
  return (
    <tr>
      <td>
        <div className="service">
          <span className="service-icon">
            <Network size={19} />
          </span>
          <div>
            <strong>{route.name}</strong>
            <small>
              {route.host}
              {route.path_prefix === '/' ? '' : route.path_prefix}
            </small>
          </div>
        </div>
      </td>
      <td>
        <code>{route.upstreams[0]}</code>
        {route.upstreams.length > 1 && <small className="more">+{route.upstreams.length - 1} more</small>}
        {live && <UpstreamHealth live={live} />}
      </td>
      <td>
        <span className={sso ? 'badge protected' : 'badge'}>
          <ShieldCheck size={12} />
          {sso ? 'SSO protected' : 'Public'}
        </span>
      </td>
      <td>
        <HttpsBadge route={route} certificate={certificate} />
      </td>
      <td>
        <span className={route.enabled ? 'route-status' : 'route-status disabled'}>
          <span className="dot" />
          {route.enabled ? 'Enabled' : 'Disabled'}
        </span>
      </td>
      <td>
        <div className="actions">
          <button className="icon" aria-label={`Edit ${route.name}`} onClick={() => onEdit(route)}>
            <Pencil size={15} />
          </button>
          <button className="icon" aria-label={`Delete ${route.name}`} onClick={() => onDelete(route)}>
            <Trash2 size={15} />
          </button>
        </div>
      </td>
    </tr>
  );
}
