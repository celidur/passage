import type {ActiveCheck, Policy} from '../../api/types';
import {DEFAULT_CHECK} from './host';

type Props = {
  policy: Policy;
  onChange: (policy: Policy) => void;
};

type NumberFieldProps = {
  label: string;
  value: number | null;
  /** Empty input reports `null`. */
  onChange: (value: number | null) => void;
  min?: number;
  placeholder?: string;
};

function NumberField({label, value, onChange, min = 1, placeholder}: NumberFieldProps) {
  return (
    <label>
      {label}
      <input
        type="number"
        min={min}
        inputMode="numeric"
        placeholder={placeholder}
        value={value ?? ''}
        onChange={(e) => onChange(e.target.value === '' ? null : Number(e.target.value))}
      />
    </label>
  );
}

/** Balancing, timeouts, limits and health settings of a host. */
export function PolicyFields({policy, onChange}: Props) {
  const {timeouts, limits, health} = policy;
  const set = (patch: Partial<Policy>) => onChange({...policy, ...patch});
  const setHealth = (patch: Partial<Policy['health']>) => set({health: {...health, ...patch}});
  const setCheck = (patch: Partial<ActiveCheck>) =>
    health.check && setHealth({check: {...health.check, ...patch}});
  const required = (apply: (value: number) => void) => (value: number | null) => apply(value ?? 0);

  return (
    <div className="policy">
      <div className="form-row halves">
        <label>
          Load balancing
          <select value={policy.balancing} onChange={(e) => set({balancing: e.target.value as Policy['balancing']})}>
            <option value="round_robin">Round robin</option>
            <option value="least_connections">Least connections</option>
          </select>
        </label>
        <label>
          Upstream protocol
          <select value={policy.protocol} onChange={(e) => set({protocol: e.target.value as Policy['protocol']})}>
            <option value="http1">HTTP/1.1</option>
            <option value="auto">Automatic (HTTP/2 over HTTPS)</option>
            <option value="http2">HTTP/2 only (gRPC)</option>
          </select>
        </label>
      </div>

      <h3>Timeouts (seconds)</h3>
      <div className="form-row thirds">
        <NumberField
          label="Connect"
          value={timeouts.connect_secs}
          onChange={required((connect_secs) => set({timeouts: {...timeouts, connect_secs}}))}
        />
        <NumberField
          label="Read"
          value={timeouts.read_secs}
          onChange={required((read_secs) => set({timeouts: {...timeouts, read_secs}}))}
        />
        <NumberField
          label="Write"
          value={timeouts.write_secs}
          onChange={required((write_secs) => set({timeouts: {...timeouts, write_secs}}))}
        />
      </div>
      <small>Read is the longest silence tolerated from the application, including streams and WebSockets.</small>

      <h3>Limits</h3>
      <div className="form-row thirds">
        <NumberField
          label="Concurrent requests"
          placeholder="No limit"
          value={limits.max_connections}
          onChange={(max_connections) => set({limits: {...limits, max_connections}})}
        />
        <NumberField
          label="Max body (bytes)"
          placeholder="No limit"
          value={limits.max_request_body_bytes}
          onChange={(max_request_body_bytes) => set({limits: {...limits, max_request_body_bytes}})}
        />
        <NumberField
          label="Retries"
          min={0}
          value={policy.retries}
          onChange={required((retries) => set({retries}))}
        />
      </div>

      <h3>Health</h3>
      <div className="form-row halves">
        <NumberField
          label="Failures before skipping"
          value={health.max_failures}
          onChange={required((max_failures) => setHealth({max_failures}))}
        />
        <NumberField
          label="Skipped for (seconds)"
          value={health.cooldown_secs}
          onChange={required((cooldown_secs) => setHealth({cooldown_secs}))}
        />
      </div>
      <label className="checkbox">
        <input
          type="checkbox"
          checked={health.check !== null}
          onChange={(e) => setHealth({check: e.target.checked ? DEFAULT_CHECK : null})}
        />
        Probe upstreams periodically
      </label>
      {health.check && (
        <>
          <div className="form-row halves">
            <label>
              Probe
              <select value={health.check.kind} onChange={(e) => setCheck({kind: e.target.value as ActiveCheck['kind']})}>
                <option value="http">HTTP GET</option>
                <option value="tcp">TCP connect (HTTP/2, gRPC)</option>
              </select>
            </label>
            <label>
              Path
              <input
                disabled={health.check.kind === 'tcp'}
                value={health.check.path}
                onChange={(e) => setCheck({path: e.target.value})}
              />
            </label>
          </div>
          <div className="form-row halves">
            <NumberField
              label="Every (seconds)"
              value={health.check.interval_secs}
              onChange={required((interval_secs) => setCheck({interval_secs}))}
            />
            <NumberField
              label="Timeout (seconds)"
              value={health.check.timeout_secs}
              onChange={required((timeout_secs) => setCheck({timeout_secs}))}
            />
          </div>
          <div className="form-row halves">
            <NumberField
              label="Failures to fail"
              value={health.check.unhealthy_threshold}
              onChange={required((unhealthy_threshold) => setCheck({unhealthy_threshold}))}
            />
            <NumberField
              label="Passes to recover"
              value={health.check.healthy_threshold}
              onChange={required((healthy_threshold) => setCheck({healthy_threshold}))}
            />
          </div>
        </>
      )}
    </div>
  );
}
