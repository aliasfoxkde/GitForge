# GitForge Resource Admission Context

This context defines the resource terms used when GitForge places work on runner hosts.

## Language

**Storage domain**:
A stable identity for a physical or shared storage resource whose contention affects job admission. A mountpoint path alone does not establish that two hosts use the same resource.
_Avoid_: mount path, host name (when naming the resource itself)

**Runner host**:
The machine that owns a runner process and executes its assigned job containers.
_Avoid_: scheduler host (when referring to the machine executing the job)

**Host admission lease**:
An exclusive reservation for resource-sensitive work on one runner host, held while that work is active.
_Avoid_: global lock, fleet-wide lease

**Resource-sensitive job**:
A job whose execution materially consumes constrained storage or build capacity and therefore participates in host admission.
_Avoid_: heavy job (unless referring specifically to the queue lane)
