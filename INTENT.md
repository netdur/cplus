# INTENT — facet and facet_appkit

This file records intent, not a plan. It is the mental model the two
packages are built against. Any work on them is judged against this page.

## facet

facet is the UI vocabulary for C+ applications.

The vocabulary is bootstrapped from MAUI, control by control: MAUI has a
button, so facet starts with a button. MAUI has a slider, so facet starts
with a slider. What is taken over does not keep MAUI's names: everything
is renamed to follow naming_guideline.md, so the vocabulary reads as C+
from the first day. That is the seed, not an ongoing authority. Once
bootstrapped, facet is its own thing: it grows by what its applications
need, and adds words of its own.

facet also carries the parts of the system that are pure C+ and belong to
no platform: the layout engine, the description language an application
writes, the agent surface. These live in facet because they are the same
everywhere.

The whole of what facet declares is the contract.

An application written against facet names no platform. It says "a button
with this title, doing this when clicked," and nothing more.

## facet_appkit

facet_appkit reads facet's contract and implements it with AppKit.

For each thing facet declares, facet_appkit either implements it with
what AppKit offers, or states plainly that AppKit cannot. The package
holds nothing else: no second vocabulary, no side door that looks
portable.

A developer remains free to drop beneath facet at will and use native
AppKit directly, for what is unique to the platform. That is a
deliberate, visible choice to write macOS-only code, made in the
application. The usual road for a missing thing stays: the application
asks facet for it, facet declares it, the backend follows.

What is said here about AppKit holds for every backend: one platform, one
package, the same rule.

## The test

Anyone can answer "what can facet do?" by reading facet's contract, and
"how does that happen on a Mac?" by reading facet_appkit, and will find
nothing in the backend that the contract did not ask for.
