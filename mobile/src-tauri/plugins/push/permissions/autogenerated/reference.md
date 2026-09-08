## Default Permission

Lets the app ask iOS for notification permission and register with APNs. The
token and a tapped alert's payload arrive as the `push://token` and
`push://opened` events, which need no permission of their own.

#### This default permission set includes the following:

- `allow-request-permission`
- `allow-register`
- `allow-unregister`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`push:allow-register`

</td>
<td>

Enables the register command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`push:deny-register`

</td>
<td>

Denies the register command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`push:allow-request-permission`

</td>
<td>

Enables the request_permission command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`push:deny-request-permission`

</td>
<td>

Denies the request_permission command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`push:allow-unregister`

</td>
<td>

Enables the unregister command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`push:deny-unregister`

</td>
<td>

Denies the unregister command without any pre-configured scope.

</td>
</tr>
</table>
