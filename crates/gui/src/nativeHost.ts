export type UnlistenFn = () => void;

type NativeRequest = {
  command: string;
  args: Record<string, unknown>;
};

type NativeEvent = {
  name: string;
  payload: unknown;
};

type NativeMessageHandler = {
  postMessage(message: NativeRequest): Promise<unknown>;
};

declare global {
  interface Window {
    mfbNative?: NativeMessageHandler;
    webkit?: {
      messageHandlers?: {
        mfb?: NativeMessageHandler;
      };
    };
    __MFB_NATIVE_EVENT__?: (event: NativeEvent) => void;
  }
}

const handler = (): NativeMessageHandler => {
  const native =
    globalThis.window.mfbNative ??
    globalThis.window.webkit?.messageHandlers?.mfb;
  if (!native) {
    throw new Error("Modern Format Boost native macOS host is unavailable");
  }
  return native;
};

// Keep the call-site result type explicit; native replies cross an untyped WebKit boundary.
export const invoke = async <T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> => (await handler().postMessage({ command, args })) as T;

// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
export const listen = <T>(
  eventName: string,
  listener: (event: { payload: T }) => void,
): Promise<UnlistenFn> => {
  const name = `mfb:${eventName}`;
  const receive = (event: Event) => {
    listener({ payload: (event as CustomEvent<T>).detail });
  };
  globalThis.addEventListener(name, receive);
  return Promise.resolve(() => {
    globalThis.removeEventListener(name, receive);
  });
};

globalThis.window.__MFB_NATIVE_EVENT__ = ({ name, payload }) => {
  globalThis.dispatchEvent(
    new CustomEvent(`mfb:${name}`, {
      detail: payload,
    }),
  );
};

const nativeWindow = {
  minimize: () => invoke<undefined>("window_minimize"),
  isMaximized: () => invoke<boolean>("window_is_maximized"),
  maximize: () => invoke<undefined>("window_maximize"),
  unmaximize: () => invoke<undefined>("window_unmaximize"),
  close: () => invoke<undefined>("window_close"),
  show: () => invoke<undefined>("window_show"),
  startDragging: () => invoke<undefined>("window_start_drag"),
};

export const getCurrentWindow = () => nativeWindow;

export const open = async (options: {
  directory?: boolean;
  multiple?: boolean;
  title?: string;
}): Promise<string | null> =>
  invoke<string | null>("open_folder", {
    directory: options.directory ?? true,
    multiple: options.multiple ?? false,
    title: options.title ?? "Select folder",
  });
