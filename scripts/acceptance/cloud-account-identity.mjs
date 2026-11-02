// Run against the rebuilt host to catch synthetic workspace identity leaking into Cloud.
import assert from 'node:assert/strict';
import {engine,artifact} from './harness.mjs';
const host=await engine('cloud-account-identity');
try {
 const account=await host.call('CloudAccountStatus');
 const workspace=await host.call('AuthStatus');
 const catalog=await host.call('CloudWorkspaces');
 assert.equal(account.user,null,'Disposable account starts signed out');
 assert.equal(workspace.state,'signedIn','Local workspace remains usable');
 assert.equal(catalog.accountId??null,account.user?.id??null,'Cloud must use the account selected in General settings');
 assert.deepEqual(catalog.workspaces,[]);
 await assert.rejects(host.call('CloudBillingSummary'),/Sign in/);
 await assert.rejects(host.call('CloudProjects',{expectedAccountId:workspace.user.id}),/Sign in/);
 artifact('cloud-account-host-acceptance.json',{rebuiltRealEngine:true,localWorkspaceUsable:true,signedOutCloudHasNoSyntheticIdentity:true,billingAndRepositoryReadsRequireGeneralAccount:true,providerOrPaymentWrites:0});
 console.log('Cloud identity host acceptance passed: local workspace remains usable; signed-out Cloud has no synthetic owner and cannot read billing/repositories.');
} finally {await host.close();}
