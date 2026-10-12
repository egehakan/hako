// Turns the HTML that Quick Look exports for an office document into a PDF, with a web view
// that is never put on screen. Run by `osascript -l JavaScript` with the export (a `.qlpreview`
// folder or one HTML file), the PDF to write and a time limit in seconds as arguments. Prints
// `ok` or `error: <reason>`.

ObjC.import('AppKit');
ObjC.import('WebKit');
ObjC.import('Quartz');

const PAGE_WIDTH = 826;

function run(argv) {
    const [input, output, seconds] = argv;
    const deadline = Date.now() + Number(seconds) * 1000;
    const spin = (interval) => $.NSRunLoop.currentRunLoop.runUntilDate(
        $.NSDate.dateWithTimeIntervalSinceNow(interval));
    const waitFor = (isDone) => {
        while (!isDone() && Date.now() < deadline) {
            spin(0.02);
        }
        return isDone();
    };

    $.NSApplication.sharedApplication.setActivationPolicy(
        $.NSApplicationActivationPolicyProhibited);
    const web = $.WKWebView.alloc.initWithFrameConfiguration(
        $.NSMakeRect(0, 0, PAGE_WIDTH, 1000), $.WKWebViewConfiguration.alloc.init);

    const documents = [];
    for (const page of pagesToConvert(input)) {
        const url = $.NSURL.fileURLWithPath(page);
        web.loadFileURLAllowingReadAccessToURL(url, url.URLByDeletingLastPathComponent);
        spin(0.05);
        if (!waitFor(() => !web.loading)) {
            return 'error: the document took too long to load';
        }

        let size = null;
        web.evaluateJavaScriptCompletionHandler(
            '[document.documentElement.scrollWidth, document.documentElement.scrollHeight].join(",")',
            (value, error) => { size = error.isNil() ? ObjC.unwrap(value) : ''; });
        if (!waitFor(() => size !== null) || size === '') {
            return 'error: the document could not be measured';
        }
        const [width, height] = size.split(',').map(Number);

        const configuration = $.WKPDFConfiguration.alloc.init;
        configuration.rect = $.NSMakeRect(0, 0, Math.max(width, 1), Math.max(height, 1));
        let result = null;
        web.createPDFWithConfigurationCompletionHandler(configuration, (data, error) => {
            result = data.isNil() ? ObjC.unwrap(error.localizedDescription) : data;
        });
        if (!waitFor(() => result !== null)) {
            return 'error: the PDF export took too long';
        }
        if (typeof result === 'string') {
            return 'error: ' + result;
        }
        documents.push(result);
    }

    if (documents.length === 0) {
        return 'error: nothing to convert';
    }
    if (documents.length === 1) {
        return documents[0].writeToFileAtomically(output, true) ? 'ok' : 'error: the PDF could not be written';
    }
    const merged = $.PDFDocument.alloc.init;
    for (const data of documents) {
        const part = $.PDFDocument.alloc.initWithData(data);
        for (let index = 0; index < part.pageCount; index++) {
            merged.insertPageAtIndex(part.pageAtIndex(index), merged.pageCount);
        }
    }
    return merged.writeToFile(output) ? 'ok' : 'error: the PDF could not be written';
}

// A spreadsheet is exported as a page of tabs around one HTML file per sheet. The sheets are
// converted one by one, in order; the page of tabs would only show the first.
function pagesToConvert(input) {
    if (input.endsWith('.html')) {
        return [input];
    }
    const properties = $.NSDictionary.dictionaryWithContentsOfFile(input + '/PreviewProperties.plist');
    const sheets = [];
    if (!properties.isNil()) {
        const attachments = properties.objectForKey('Attachments');
        if (!attachments.isNil()) {
            for (const name of ObjC.deepUnwrap(attachments.allKeys)) {
                const match = /(?:^|\/)Sheet(\d+)\.html$/.exec(name);
                if (!match) {
                    continue;
                }
                const file = attachments.objectForKey(name).objectForKey('DumpedAttachmentFileName');
                if (!file.isNil()) {
                    sheets.push({ index: Number(match[1]), path: input + '/' + ObjC.unwrap(file) });
                }
            }
        }
    }
    if (sheets.length === 0) {
        return [input + '/Preview.html'];
    }
    return sheets.sort((a, b) => a.index - b.index).map((sheet) => sheet.path);
}
