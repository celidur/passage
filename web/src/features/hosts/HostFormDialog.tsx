import {useState, type FormEvent} from 'react';
import {ArrowRight, X} from 'lucide-react';
import type {Route, RouteInput} from '../../api/types';
import {ErrorMessage} from '../../components/Alert';
import {Dialog} from '../../components/Dialog';
import {useAsyncAction} from '../../hooks/useAsyncAction';
import {EMPTY_HOST_FORM, toForm, toInput, type HostForm} from './host';

type Props = {
  /** `null` creates a new host. */
  route: Route | null;
  onSave: (input: RouteInput) => Promise<void>;
  onClose: () => void;
};

export function HostFormDialog({route, onSave, onClose}: Props) {
  const [form, setForm] = useState<HostForm>(() => (route ? toForm(route) : EMPTY_HOST_FORM));
  const {busy, error, run} = useAsyncAction();
  const update = (patch: Partial<HostForm>) => setForm((current) => ({...current, ...patch}));

  function submit(event: FormEvent) {
    event.preventDefault();
    void run(() => onSave(toInput(form)));
  }

  return (
    <Dialog titleId="host-dialog-title">
      <div className="dialog-head">
        <div>
          <div className="eyebrow">CONFIGURE YOUR ENTRANCE</div>
          <h2 id="host-dialog-title">{route ? 'Edit proxy host' : 'Add proxy host'}</h2>
        </div>
        <button className="icon" aria-label="Close dialog" onClick={onClose}>
          <X size={20} />
        </button>
      </div>
      <form onSubmit={submit}>
        <label>
          Service name
          <input
            autoFocus
            required
            maxLength={100}
            placeholder="My application"
            value={form.name}
            onChange={(e) => update({name: e.target.value})}
          />
        </label>
        <div className="form-row">
          <label>
            Domain
            <input
              required
              placeholder="app.example.com"
              value={form.host}
              onChange={(e) => update({host: e.target.value})}
            />
          </label>
          <label>
            Path prefix
            <input required value={form.path_prefix} onChange={(e) => update({path_prefix: e.target.value})} />
          </label>
        </div>
        <label>
          Upstream origin
          <input type="url" required value={form.upstream} onChange={(e) => update({upstream: e.target.value})} />
        </label>
        <small>Full request path is preserved. DNS resolves when the route is saved.</small>
        <div className="auth-section">
          <label>
            Forward authentication URL <span>OPTIONAL</span>
            <input
              type="url"
              placeholder="https://auth.example.com/verify"
              value={form.forward_auth_url}
              onChange={(e) => update({forward_auth_url: e.target.value})}
            />
          </label>
          <small>A successful response grants access. Errors deny access.</small>
        </div>
        <label className="checkbox">
          <input type="checkbox" checked={form.enabled} onChange={(e) => update({enabled: e.target.checked})} />
          Enable this host
        </label>
        <ErrorMessage>{error}</ErrorMessage>
        <div className="dialog-foot">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={busy}>
            {busy ? 'Validating…' : 'Save proxy host'}
            <ArrowRight size={16} />
          </button>
        </div>
      </form>
    </Dialog>
  );
}
