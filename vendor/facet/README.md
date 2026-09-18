# facet

The platform-neutral UI vocabulary and tree used by every facet backend.

Add it with `cpc pm add . facet`, which writes its dependency closure:

```toml
[dependencies]
facet = "*"
```

Then import the modules you use:

```cplus
import "facet/facet" as facet;
import "facet/elements" as ui;

fn content() -> facet::Node {
    var column: facet::Builder = facet::Builder::new();
    column.add(ui::label("Hello", key: "greeting"));
    return ui::column(column, key: "content");
}
```

facet describes UI and owns its state; a backend such as `facet_appkit`,
`facet_gtk`, `facet_uikit`, or `facet_win32` mounts the tree.

- [Tutorial](docs/tutorial.md) — build and run a first screen
- [Guide](docs/guide.md) — tree, ownership, seams, and gotchas
- [Reference](docs/ref.md) — hand-written public surface
- [Control contract](docs/contract.md) — generated control verbs
- [Navigation](docs/navigation.md) — apps, windows, routes, and lifecycles

Run unit tests from this directory with `cpc test`.
