import {useState, type FormEvent} from 'react';
import {ArrowRight, X} from 'lucide-react';
import type {Route, RouteInput, Status} from '../../api/types';
import {ErrorMessage} from '../../components/Alert';
import {Dialog} from '../../components/Dialog';
import {useAsyncAction} from '../../hooks/useAsyncAction';
import {EMPTY_HOST_FORM, toForm, toInput, type HostForm} from './host';
import {PolicyFields} from './PolicyFields';

type Props = {
  /** `null` creates a new host. */
  route: Route | null;
  /** What the server can do about HTTPS; unknown until the status has loaded. */
  https: Status['https'] | undefined;
  onSave: (input: RouteInput) => Promise<void>;
  onClose: () => void;
};

/** What enabling HTTPS will do, given how the server is configured. */
function httpsHint(https: Status['https'] | undefined) {
  if (!https) return '';
  if (!https.enabled) return 'Add an [https] section to passage.toml to serve HTTPS.';
  if (!https.acme) {
    return 'Automatic certificates are not configured. A certificate must already cover this domain, or the fallback certificate is used.';
  }
  return 'A certificate is requested for this domain (HTTP-01) and renewed automatically. The domain must point to this server, and port 80 must reach Passage.';
}

export function HostFormDialog({route, https, onSave, onClose}: Props) {
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
          Upstream origins <span className="hint">ONE PER LINE</span>
          <textarea
            required
            rows={2}
            spellCheck={false}
            placeholder={'http://10.0.0.5:3000\nhttp://10.0.0.6:3000'}
            value={form.upstreams}
            onChange={(e) => update({upstreams: e.target.value})}
          />
        </label>
        <small>
          Traffic is shared between origins. The full request path is preserved. Host names are looked up
          again periodically.
        </small>
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
        <div className="https-section">
          <label className="checkbox">
            <input
              type="checkbox"
              checked={form.https.enabled}
              disabled={https?.enabled === false}
              onChange={(e) => update({https: {enabled: e.target.checked, redirect: e.target.checked && form.https.redirect}})}
            />
            Serve over HTTPS <span className="hint">AUTOMATIC CERTIFICATE</span>
          </label>
          {form.https.enabled && (
            <label className="checkbox">
              <input
                type="checkbox"
                checked={form.https.redirect}
                onChange={(e) => update({https: {...form.https, redirect: e.target.checked}})}
              />
              Redirect HTTP to HTTPS
            </label>
          )}
          <small>{httpsHint(https)}</small>
        </div>
        <details className="advanced">
          <summary>Advanced: balancing, timeouts, limits and health</summary>
          <PolicyFields policy={form.policy} onChange={(policy) => update({policy})} />
        </details>
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
