import {Network, Pencil, ShieldCheck, Trash2} from 'lucide-react';
import type {Route} from '../../api/types';
import {isProtected} from './host';

type Props = {
  routes: Route[];
  onEdit: (route: Route) => void;
  onDelete: (route: Route) => void;
};

export function HostsTable({routes, onEdit, onDelete}: Props) {
  return (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th>SERVICE / DOMAIN</th>
            <th>UPSTREAM</th>
            <th>ACCESS</th>
            <th>STATUS</th>
            <th>
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {routes.map((route) => (
            <HostRow key={route.id} route={route} onEdit={onEdit} onDelete={onDelete} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

type RowProps = {route: Route; onEdit: (route: Route) => void; onDelete: (route: Route) => void};

function HostRow({route, onEdit, onDelete}: RowProps) {
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
        <code>{route.upstream}</code>
      </td>
      <td>
        <span className={sso ? 'badge protected' : 'badge'}>
          <ShieldCheck size={12} />
          {sso ? 'SSO protected' : 'Public'}
        </span>
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
