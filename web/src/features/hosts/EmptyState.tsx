import {Network, Plus} from 'lucide-react';

export function EmptyState({searching, onAdd}: {searching: boolean; onAdd: () => void}) {
  return (
    <div className="empty">
      <span className="empty-icon">
        <Network size={28} />
      </span>
      <h3>{searching ? 'No matching hosts' : 'Your first service starts here'}</h3>
      <p>
        {searching
          ? 'Try another service name or domain.'
          : 'Connect a domain to an upstream. Passage handles the path.'}
      </p>
      {!searching && (
        <button className="secondary" onClick={onAdd}>
          <Plus size={15} />
          Add your first proxy host
        </button>
      )}
    </div>
  );
}
