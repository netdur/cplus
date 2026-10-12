// Drop files on a point of the screen, the way Explorer does: a shell data
// object for the files (CF_HDROP and the shell's own formats, what a WinUI
// DataPackageView reads as StorageItems) and an OLE DoDragDrop whose source
// walks the cursor from where it is to the target, rests there, and drops.
//
//   winui_drop_files.exe [--via VX VY] X Y FILE...
//
// Physical pixels and absolute paths. With --via the cursor rests over
// (VX, VY) first and then walks on to (X, Y): a drag that crosses a target
// and leaves it.
//
// Prints the effect the target answered - 1 (copy) when it took the files,
// 0 when it refused them - and exits 0 if the drag ran at all.
//
// A DRAG STARTS WITH A PRESS ON THE SOURCE, and so does this one. The drag
// loop sees the mouse only through its capture, and the capture belongs to
// the window the button went down on. So a small window of this process is
// put under the cursor, the left button is pressed on it, and DoDragDrop runs
// from its WM_LBUTTONDOWN, holding the button for the whole walk. The walk is
// another thread's: the loop wakes only for input, which that thread makes.
//
// Built by tools/test_facet_runtime_winui.py with clang:
//   clang -O1 tools/winui_drop_files.c -o winui_drop_files.exe -lole32 -lshell32 -luser32 -luuid -lgdi32
#define COBJMACROS
#include <windows.h>
#include <ole2.h>
#include <shlobj.h>
#include <shobjidl.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct { IDropSource iface; LONG refs; volatile LONG done; POINT from, via, to; BOOL has_via; } Source;

static const int WALK = 30;   // moves on a leg, 15 ms apart
static const int REST = 40;   // moves resting at its end

static Source source;
static IDataObject *data;
static HRESULT result = E_FAIL;
static DWORD effect = DROPEFFECT_NONE;

static void button(DWORD flag) {
    INPUT in = {0};
    in.type = INPUT_MOUSE;
    in.mi.dwFlags = flag;
    SendInput(1, &in, sizeof in);
}
static void move_to(int x, int y) {
    // Through the input queue, as a person's mouse arrives.
    INPUT in = {0};
    in.type = INPUT_MOUSE;
    in.mi.dx = MulDiv(x, 65535, GetSystemMetrics(SM_CXSCREEN) - 1);
    in.mi.dy = MulDiv(y, 65535, GetSystemMetrics(SM_CYSCREEN) - 1);
    in.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE;
    SendInput(1, &in, sizeof in);
}

static HRESULT STDMETHODCALLTYPE qi(IDropSource *self, REFIID riid, void **out) {
    if (IsEqualIID(riid, &IID_IUnknown) || IsEqualIID(riid, &IID_IDropSource)) {
        *out = self; self->lpVtbl->AddRef(self); return S_OK;
    }
    *out = NULL; return E_NOINTERFACE;
}
static ULONG STDMETHODCALLTYPE add_ref(IDropSource *self) { return InterlockedIncrement(&((Source *)self)->refs); }
static ULONG STDMETHODCALLTYPE release(IDropSource *self) { return InterlockedDecrement(&((Source *)self)->refs); }
static HRESULT STDMETHODCALLTYPE query_continue(IDropSource *self, BOOL escape, DWORD keys) {
    return ((Source *)self)->done ? DRAGDROP_S_DROP : S_OK;
}
static HRESULT STDMETHODCALLTYPE give_feedback(IDropSource *self, DWORD effect) { return DRAGDROP_S_USEDEFAULTCURSORS; }
static IDropSourceVtbl SOURCE_VTBL = { qi, add_ref, release, query_continue, give_feedback };

// Each leg, then a rest at its end (a target sees DragOver there).
static void leg(POINT a, POINT b) {
    for (int i = 1; i <= WALK; i++) { move_to(a.x + (b.x - a.x) * i / WALK, a.y + (b.y - a.y) * i / WALK); Sleep(15); }
    for (int i = 0; i < REST; i++) { move_to(b.x + (i & 1), b.y); Sleep(15); }
}
static DWORD WINAPI walk(void *context) {
    Sleep(100);
    if (source.has_via) { leg(source.from, source.via); leg(source.via, source.to); } else leg(source.from, source.to);
    InterlockedExchange(&source.done, 1);
    button(MOUSEEVENTF_LEFTUP);    // the drop, and the input that wakes the loop to see it
    return 0;
}

static LRESULT CALLBACK proc(HWND hwnd, UINT msg, WPARAM w, LPARAM l) {
    if (msg == WM_LBUTTONDOWN) {
        // Out of the way of the walk: a window under the cursor would be the
        // first drop target the drag meets.
        ShowWindow(hwnd, SW_HIDE);
        HANDLE walker = CreateThread(NULL, 0, walk, NULL, 0, NULL);
        result = DoDragDrop(data, &source.iface, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &effect);
        WaitForSingleObject(walker, INFINITE);
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, w, l);
}

int wmain(int argc, wchar_t **argv) {
    source.iface.lpVtbl = &SOURCE_VTBL;
    source.refs = 1;
    if (argc > 3 && wcscmp(argv[1], L"--via") == 0) {
        source.has_via = TRUE;
        source.via.x = _wtoi(argv[2]);
        source.via.y = _wtoi(argv[3]);
        argv += 3; argc -= 3;
    }
    if (argc < 4) { fwprintf(stderr, L"usage: winui_drop_files [--via VX VY] X Y FILE...\n"); return 2; }
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (FAILED(OleInitialize(NULL))) { fwprintf(stderr, L"OleInitialize failed\n"); return 3; }
    int count = argc - 3;
    PIDLIST_ABSOLUTE *ids = calloc(count, sizeof *ids);
    for (int i = 0; i < count; i++) {
        HRESULT hr = SHParseDisplayName(argv[3 + i], NULL, &ids[i], 0, NULL);
        if (FAILED(hr)) { fwprintf(stderr, L"no such file: %ls (0x%08lx)\n", argv[3 + i], hr); return 4; }
    }
    IShellItemArray *items = NULL;
    HRESULT hr = SHCreateShellItemArrayFromIDLists(count, (PCIDLIST_ABSOLUTE_ARRAY)ids, &items);
    if (FAILED(hr)) { fwprintf(stderr, L"shell item array 0x%08lx\n", hr); return 5; }
    hr = IShellItemArray_BindToHandler(items, NULL, &BHID_DataObject, &IID_IDataObject, (void **)&data);
    if (FAILED(hr)) { fwprintf(stderr, L"data object 0x%08lx\n", hr); return 6; }
    source.to.x = _wtoi(argv[1]);
    source.to.y = _wtoi(argv[2]);

    // The source window, under the cursor, and the press on it.
    GetCursorPos(&source.from);
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"winui_drop_files";
    wc.hbrBackground = (HBRUSH)GetStockObject(GRAY_BRUSH);
    RegisterClassW(&wc);
    HWND hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, wc.lpszClassName, L"", WS_POPUP | WS_VISIBLE,
                                source.from.x - 16, source.from.y - 16, 32, 32, NULL, NULL, wc.hInstance, NULL);
    if (!hwnd) { fwprintf(stderr, L"no source window\n"); return 8; }
    UpdateWindow(hwnd);
    Sleep(200);
    button(MOUSEEVENTF_LEFTDOWN);
    MSG msg;
    SetTimer(NULL, 0, 30000, NULL);   // a press that never arrives ends the run
    while (GetMessageW(&msg, NULL, 0, 0) > 0) {
        if (msg.message == WM_TIMER && msg.hwnd == NULL) { button(MOUSEEVENTF_LEFTUP); fwprintf(stderr, L"the press never reached the source window\n"); return 9; }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    DestroyWindow(hwnd);
    wprintf(L"%lu\n", (unsigned long)effect);
    fwprintf(stderr, L"DoDragDrop 0x%08lx\n", result);
    IDataObject_Release(data);
    IShellItemArray_Release(items);
    for (int i = 0; i < count; i++) CoTaskMemFree((void *)ids[i]);
    free(ids);
    OleUninitialize();
    return (result == DRAGDROP_S_DROP || result == DRAGDROP_S_CANCEL) ? 0 : 7;
}
