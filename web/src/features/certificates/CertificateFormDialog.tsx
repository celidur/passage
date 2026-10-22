import {useState, type FormEvent} from 'react';
import {ArrowRight, X} from 'lucide-react';
import type {CertificateChallenge, NewCertificate} from '../../api/types';
import {ErrorMessage} from '../../components/Alert';
import {Dialog} from '../../components/Dialog';
import {useAsyncAction} from '../../hooks/useAsyncAction';

type Props = {
  /** Whether a DNS hook is configured, which wildcard certificates need. */
  dnsAvailable: boolean;
  onSave: (input: NewCertificate) => Promise<void>;
  onClose: () => void;
};

export function CertificateFormDialog({dnsAvailable, onSave, onClose}: Props) {
  const [domains, setDomains] = useState('');
  const [challenge, setChallenge] = useState<CertificateChallenge>(dnsAvailable ? 'dns-01' : 'http-01');
  const {busy, error, run} = useAsyncAction();

  function submit(event: FormEvent) {
    event.preventDefault();
    const list = domains
      .split(/[\s,]+/)
      .map((domain) => domain.trim())
      .filter(Boolean);
    void run(() => onSave({domains: list, challenge}));
  }

  return (
    <Dialog titleId="certificate-dialog-title">
      <div className="dialog-head">
        <div>
          <div className="eyebrow">HTTPS, ON AUTOPILOT</div>
          <h2 id="certificate-dialog-title">Add certificate</h2>
        </div>
        <button className="icon" aria-label="Close dialog" onClick={onClose}>
          <X size={20} />
        </button>
      </div>
      <form onSubmit={submit}>
        <label>
          Domains <span className="hint">ONE PER LINE</span>
          <textarea
            autoFocus
            required
            rows={3}
            spellCheck={false}
            placeholder={'*.example.com\nexample.com'}
            value={domains}
            onChange={(e) => setDomains(e.target.value)}
          />
        </label>
        <label>
          Validation
          <select value={challenge} onChange={(e) => setChallenge(e.target.value as CertificateChallenge)}>
            <option value="http-01">HTTP-01: serve a proof on port 80</option>
            <option value="dns-01" disabled={!dnsAvailable}>
              DNS-01: publish a TXT record (wildcards)
            </option>
          </select>
        </label>
        <small>
          {dnsAvailable
            ? 'Wildcards such as *.example.com need DNS-01. Passage publishes the record with the configured DNS hook.'
            : 'DNS-01 and wildcard certificates need acme.dns_hook in passage.toml.'}{' '}
          Enabling HTTPS on a host orders its certificate automatically; use this for wildcards or several names on one
          certificate.
        </small>
        <ErrorMessage>{error}</ErrorMessage>
        <div className="dialog-foot">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button className="primary" disabled={busy}>
            {busy ? 'Requesting…' : 'Request certificate'}
            <ArrowRight size={16} />
          </button>
        </div>
      </form>
    </Dialog>
  );
}
