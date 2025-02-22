import { ProcessEvents, PromiseHandlers } from "../types";
import { ConfluxConfig } from "../../conflux";
export declare class ProcessManager {
    private readonly events;
    private process;
    private isRunning;
    constructor(events?: ProcessEvents);
    start: (config: ConfluxConfig) => Promise<void>;
    setupListeners: (handle: PromiseHandlers) => void;
    cleanup: () => Promise<void>;
    stop: () => Promise<void>;
    setupStopListeners: (handle: PromiseHandlers) => void;
}
