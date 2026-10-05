param([ValidateSet('inspect','navigate','invoke','toggle','value','range','scroll','focus')][string]$Action='inspect',[string]$Id='',[string]$Value='')
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root=[System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,'Facet Gallery / WinUI'))
if (!$root) {throw 'Gallery window not found'}
if ($Action -eq 'navigate') {
    $condition=[System.Windows.Automation.AndCondition]::new([System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty,$Id),[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[System.Windows.Automation.ControlType]::TreeItem))
    $item=$root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
    if ($Value -eq 'last') {
        $navigationMatches=$root.FindAll([System.Windows.Automation.TreeScope]::Descendants,$condition)
        if ($navigationMatches.Count) {$item=$navigationMatches.Item($navigationMatches.Count-1)}
    }
    if (!$item) {
        $nav=$root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[System.Windows.Automation.ControlType]::Tree))
        $scroll=[System.Windows.Automation.ScrollPattern]$nav.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
        foreach ($percent in @(0,20,40,60,80,100)) {
            $scroll.SetScrollPercent(-1,$percent)
            Start-Sleep -Milliseconds 100
            $item=$root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,$condition)
            if ($item) {break}
        }
    }
    if (!$item) {throw "Navigation item not found: $Id"}
    $pattern=$null
    if ($item.TryGetCurrentPattern([System.Windows.Automation.ScrollItemPattern]::Pattern,[ref]$pattern)) {$pattern.ScrollIntoView()}
    ([System.Windows.Automation.InvokePattern]$item.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)).Invoke()
} elseif ($Action -ne 'inspect') {
    $item=$root.FindFirst([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty,$Id))
    if (!$item) {throw "Control not found: $Id"}
    switch ($Action) {
        'invoke' {
            $activationPattern=$null
            if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$activationPattern)) {
                ([System.Windows.Automation.InvokePattern]$activationPattern).Invoke()
            } elseif ($item.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern,[ref]$activationPattern)) {
                ([System.Windows.Automation.TogglePattern]$activationPattern).Toggle()
            } else {throw "Control exposes neither Invoke nor Toggle: $Id"}
        }
        'focus' {$item.SetFocus()}
        'toggle' {([System.Windows.Automation.TogglePattern]$item.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)).Toggle()}
        'value' {([System.Windows.Automation.ValuePattern]$item.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)).SetValue($Value)}
        'range' {([System.Windows.Automation.RangeValuePattern]$item.GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern)).SetValue([double]$Value)}
        'scroll' {([System.Windows.Automation.ScrollPattern]$item.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)).SetScrollPercent(-1,[double]$Value)}
    }
}
Start-Sleep -Milliseconds 200
$nodes=$root.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
$result=@(foreach ($node in $nodes) {
    $c=$node.Current
    $b=$c.BoundingRectangle
    if ($c.AutomationId -or $c.Name) {
        $value=$null;$p=$null
        if (!$c.IsPassword -and $node.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern,[ref]$p)) {$value=([System.Windows.Automation.ValuePattern]$p).Current.Value}
        $toggle=$null;$p=$null
        if ($node.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern,[ref]$p)) {$toggle=([System.Windows.Automation.TogglePattern]$p).Current.ToggleState.ToString()}
        @{id=$c.AutomationId;name=$c.Name;type=$c.ControlType.ProgrammaticName;enabled=$c.IsEnabled;offscreen=$c.IsOffscreen;focused=$c.HasKeyboardFocus;password=$c.IsPassword;value=$value;toggle=$toggle;bounds=@($b.X,$b.Y,$b.Width,$b.Height)}
    }
})
ConvertTo-Json -InputObject $result -Depth 4 -Compress
