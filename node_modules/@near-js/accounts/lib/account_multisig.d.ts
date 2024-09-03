import { Action } from '@near-js/transactions';
import { FinalExecutionOutcome } from '@near-js/types';
import { Account, SignAndSendTransactionOptions } from './account';
import { Connection } from './connection';
import { MultisigStateStatus } from './types';
declare enum MultisigCodeStatus {
    INVALID_CODE = 0,
    VALID_CODE = 1,
    UNKNOWN_CODE = 2
}
export declare class AccountMultisig extends Account {
    storage: any;
    onAddRequestResult: (any: any) => any;
    constructor(connection: Connection, accountId: string, options: any);
    signAndSendTransactionWithAccount(receiverId: string, actions: Action[]): Promise<FinalExecutionOutcome>;
    signAndSendTransaction({ receiverId, actions }: SignAndSendTransactionOptions): Promise<FinalExecutionOutcome>;
    checkMultisigCodeAndStateStatus(contractBytes?: Uint8Array): Promise<{
        codeStatus: MultisigCodeStatus;
        stateStatus: MultisigStateStatus;
    }>;
    deleteRequest(request_id: any): Promise<FinalExecutionOutcome>;
    deleteAllRequests(): Promise<void>;
    deleteUnconfirmedRequests(): Promise<void>;
    getRequestIds(): Promise<string[]>;
    getRequest(): any;
    setRequest(data: any): any;
}
export {};
